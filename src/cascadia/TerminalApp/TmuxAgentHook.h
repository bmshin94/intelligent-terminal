// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#pragma once

#include "TmuxProtocol.h"
#include "../../tools/wtcli/wtcli_functions.h"

#include <memory>
#include <optional>

namespace Microsoft::Terminal::Tmux
{
    inline constexpr std::string_view AgentHookPrefix{ "IT_AGENT_HOOK/1 " };
    inline constexpr size_t MaxAgentHookMessageBytes = 64 * 1024;

    struct AgentHookMessage
    {
        Id sessionId{};
        Id paneId{};
        std::string cliSource;
        std::string event;
        std::string payload;
    };

    inline std::optional<AgentHookMessage> ParseAgentHookMessage(const std::string_view message)
    {
        if (!message.starts_with("IT_AGENT_HOOK/"))
        {
            return std::nullopt;
        }
        if (!message.starts_with(AgentHookPrefix) || message.size() > MaxAgentHookMessageBytes ||
            !std::all_of(message.begin(), message.end(), [](const unsigned char ch) { return ch >= 32 && ch < 127; }))
        {
            throw ProtocolError{ "Invalid tmux agent hook version, size or encoding" };
        }

        Json::CharReaderBuilder builder;
        builder["collectComments"] = false;
        builder["allowComments"] = false;
        builder["failIfExtra"] = true;
        builder["rejectDupKeys"] = true;
        builder["stackLimit"] = 64;
        const std::unique_ptr<Json::CharReader> reader{ builder.newCharReader() };
        Json::Value body;
        std::string errors;
        bool parsed = false;
        try
        {
            parsed = reader->parse(message.data() + AgentHookPrefix.size(), message.data() + message.size(), &body, &errors);
        }
        catch (const Json::Exception&)
        {
            throw ProtocolError{ "Invalid tmux agent hook JSON nesting" };
        }
        if (!parsed || !body.isObject())
        {
            throw ProtocolError{ "Invalid tmux agent hook JSON" };
        }
        for (const auto* key : { "session_id", "pane_id", "cli_source", "event" })
        {
            if (!body[key].isString())
            {
                throw ProtocolError{ "Missing tmux agent hook routing field" };
            }
        }

        AgentHookMessage hook;
        const auto session = body["session_id"].asString();
        if (session.size() < 2 || session.front() != '$')
        {
            throw ProtocolError{ "Invalid tmux agent hook session ID" };
        }
        hook.sessionId = details::Number(std::string_view{ session }.substr(1));
        hook.paneId = details::PaneId(body["pane_id"].asString());
        hook.cliSource = body["cli_source"].asString();
        hook.event = body["event"].asString();
        constexpr std::string_view sources[]{ "claude", "copilot", "codex", "gemini", "opencode" };
        constexpr std::string_view events[]{
            "agent.session.start", "agent.session.end", "agent.prompt.submit", "agent.notification", "agent.tool.starting", "agent.stop", "agent.error", "agent.subagent.stop"
        };
        if (std::find(std::begin(sources), std::end(sources), hook.cliSource) == std::end(sources) ||
            std::find(std::begin(events), std::end(events), hook.event) == std::end(events))
        {
            throw ProtocolError{ "Unsupported tmux agent hook source or event" };
        }
        const auto& payload = body["payload"];
        if (!payload.isNull() && !payload.isObject())
        {
            throw ProtocolError{ "Invalid tmux agent hook payload" };
        }
        if (payload.isObject())
        {
            for (const auto* key : { "session_id", "sessionId" })
            {
                if (payload.isMember(key) && (!payload[key].isString() || payload[key].asString().size() > 1024))
                {
                    throw ProtocolError{ "Invalid tmux agent session ID" };
                }
            }
        }
        Json::StreamWriterBuilder writer;
        writer["indentation"] = "";
        hook.payload = Json::writeString(writer, payload);
        return hook;
    }

    inline Json::Value BuildAgentHookParams(const AgentHookMessage& hook,
                                            const std::string& paneId,
                                            const std::string& tabId,
                                            const std::string& windowId,
                                            const std::string& sessionName,
                                            const std::string& socketPath)
    {
        Json::Value event;
        // Reuse the native bridge's redaction and wire budget, not a second
        // permissive path from remote hook JSON to the COM broadcast.
        if (tabId.empty() || windowId.empty() ||
            !wtcli::BuildAgentHookEventJson(hook.event, hook.cliSource, hook.payload, paneId, {}, event))
        {
            throw ProtocolError{ "Cannot normalize tmux agent hook" };
        }
        auto& params = event["params"];
        params["tab_id"] = tabId;
        params["window_id"] = windowId;
        auto& tmux = params["tmux"];
        tmux["session_id"] = "$" + std::to_string(hook.sessionId);
        tmux["pane_id"] = "%" + std::to_string(hook.paneId);
        tmux["session_name"] = wtcli::ClampUtf8(sessionName, 512);
        tmux["socket_path"] = wtcli::ClampUtf8(socketPath, 512);

        Json::StreamWriterBuilder writer;
        writer["indentation"] = "";
        const auto size = Json::writeString(writer, event).size();
        if (size > wtcli::kMaxHookEventChars)
        {
            params["payload"] = wtcli::ReduceOversizedHookPayload(params["payload"], size);
            if (Json::writeString(writer, event).size() > wtcli::kMaxHookEventChars)
            {
                throw ProtocolError{ "Tmux agent hook exceeds event budget" };
            }
        }
        return std::move(params);
    }
}
