// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "precomp.h"
#include "../TerminalApp/TmuxAgentHook.h"

using namespace WEX::TestExecution;
using namespace Microsoft::Terminal::Tmux;

namespace TerminalAppUnitTests
{
    class TmuxAgentHookTests
    {
        TEST_CLASS(TmuxAgentHookTests);
        TEST_METHOD(ParsesLiteralMessageAtEveryFragmentBoundary);
        TEST_METHOD(RejectsMalformedOrUnsupportedMessages);
        TEST_METHOD(NormalizesAndRedactsNativeEnvelope);
        TEST_METHOD(BoundsEntireEnvelopeIncludingTmuxMetadata);
    };

    void TmuxAgentHookTests::ParsesLiteralMessageAtEveryFragmentBoundary()
    {
        const std::string message = R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"copilot","event":"agent.notification","payload":{"session_id":"sid","message":"\u4f60\u597d\n#{session_name} %s \\ \u001b"}})";
        const auto wire = "%message " + message + "\n";
        for (size_t split = 0; split <= wire.size(); ++split)
        {
            Parser parser;
            auto events = parser.Feed(std::string_view{ wire }.substr(0, split));
            const auto remaining = parser.Feed(std::string_view{ wire }.substr(split));
            events.insert(events.end(), remaining.begin(), remaining.end());
            parser.Finish();
            VERIFY_ARE_EQUAL(size_t{ 1 }, events.size());
            VERIFY_IS_TRUE(events[0].kind == Event::Kind::Notification);
            VERIFY_ARE_EQUAL(std::string{ "message" }, events[0].name);
            const auto hook = ParseAgentHookMessage(events[0].text);
            VERIFY_IS_TRUE(hook.has_value());
            VERIFY_ARE_EQUAL(Id{ 7 }, hook->sessionId);
            VERIFY_ARE_EQUAL(Id{ 3 }, hook->paneId);
            const auto params = BuildAgentHookParams(*hook, "native-pane", "native-tab", "9", "work", "/socket");
            VERIFY_ARE_EQUAL(std::string{ "\xe4\xbd\xa0\xe5\xa5\xbd\n#{session_name} %s \\ \x1b" },
                             params["payload"]["message"].asString());
        }
    }

    void TmuxAgentHookTests::RejectsMalformedOrUnsupportedMessages()
    {
        VERIFY_IS_FALSE(ParseAgentHookMessage("ordinary tmux message").has_value());
        const std::string valid = R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"copilot","event":"agent.stop","payload":null})";
        VERIFY_IS_TRUE(ParseAgentHookMessage(valid).has_value());
        for (const auto text : {
                 "IT_AGENT_HOOK/2 {}", "IT_AGENT_HOOK/1 []", "IT_AGENT_HOOK/1 {}", "IT_AGENT_HOOK/1 {", R"(IT_AGENT_HOOK/1 {"session_id":"$7","session_id":"$8"})", R"(IT_AGENT_HOOK/1 {"session_id":7,"pane_id":"%3","cli_source":"copilot","event":"agent.stop"})", R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3;kill-server","cli_source":"copilot","event":"agent.stop"})", R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"custom:command","event":"agent.stop"})", R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"copilot","event":"send_input"})", R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"copilot","event":"agent.stop","payload":[]})", R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"copilot","event":"agent.stop","payload":{"session_id":2}})" })
        {
            VERIFY_THROWS(ParseAgentHookMessage(text), ProtocolError);
        }
        VERIFY_THROWS(ParseAgentHookMessage(valid + "{}"), ProtocolError);
        VERIFY_THROWS(ParseAgentHookMessage(valid + "\n%exit\n"), ProtocolError);
        VERIFY_THROWS(ParseAgentHookMessage(valid + std::string(MaxAgentHookMessageBytes, ' ')), ProtocolError);
        VERIFY_THROWS(ParseAgentHookMessage(
                          std::string{ R"(IT_AGENT_HOOK/1 {"nested":)" } + std::string(100, '[') + "0" + std::string(100, ']') + "}"),
                      ProtocolError);
        VERIFY_THROWS(ParseAgentHookMessage(
                          std::string{ R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"copilot","event":"agent.stop","payload":{"session_id":")" } +
                          std::string(1025, 's') + R"("}})"),
                      ProtocolError);
    }

    void TmuxAgentHookTests::NormalizesAndRedactsNativeEnvelope()
    {
        const auto hook = ParseAgentHookMessage(
            R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"claude","event":"agent.prompt.submit","tab_id":"forged","window_id":"forged","payload":{"session_id":"sid","cwd":"/repo","prompt":"secret","transcript_path":"/secret","model":"secret","messages":["secret"],"tool_result":"secret","tool_name":"shell","tool_input":{"command":"secret"}}})");
        const auto params = BuildAgentHookParams(*hook, "native-pane", "native-tab", "9", "work", "/socket");
        VERIFY_ARE_EQUAL(std::string{ "native-pane" }, params["pane_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "native-tab" }, params["tab_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "9" }, params["window_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "sid" }, params["agent_session_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "$7" }, params["tmux"]["session_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "%3" }, params["tmux"]["pane_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "/repo" }, params["payload"]["cwd"].asString());
        for (const auto* key : { "prompt", "transcript_path", "model", "messages", "tool_result", "tool_input" })
        {
            VERIFY_IS_FALSE(params["payload"].isMember(key));
        }
        VERIFY_THROWS(BuildAgentHookParams(*hook, {}, "tab", "9", {}, {}), ProtocolError);
        VERIFY_THROWS(BuildAgentHookParams(*hook, "pane", {}, "9", {}, {}), ProtocolError);
    }

    void TmuxAgentHookTests::BoundsEntireEnvelopeIncludingTmuxMetadata()
    {
        auto hook = *ParseAgentHookMessage(
            R"(IT_AGENT_HOOK/1 {"session_id":"$7","pane_id":"%3","cli_source":"gemini","event":"agent.notification","payload":null})");
        hook.payload = R"({"session_id":"sid","message":")" + std::string(8100, 'x') + R"(","cwd":"/repo"})";
        const auto params = BuildAgentHookParams(hook, "pane", "tab", "9", std::string(2000, 's'), std::string(2000, 'p'));
        Json::Value event;
        event["type"] = "event";
        event["method"] = "agent_event";
        event["params"] = params;
        Json::StreamWriterBuilder writer;
        writer["indentation"] = "";
        VERIFY_IS_TRUE(Json::writeString(writer, event).size() <= wtcli::kMaxHookEventChars);
        VERIFY_ARE_EQUAL(std::string{ "sid" }, params["agent_session_id"].asString());
        VERIFY_ARE_EQUAL(std::string{ "/repo" }, params["payload"]["cwd"].asString());
        VERIFY_IS_TRUE(params["payload"]["_truncated"].asBool());
    }
}
