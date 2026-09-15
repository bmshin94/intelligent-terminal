// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "precomp.h"

#include "../TerminalApp/AgentRuntimeConfig.h"

using namespace WEX::TestExecution;
using namespace WEX::Common;
namespace Config = ::TerminalApp::AgentRuntimeConfig;

namespace TerminalAppUnitTests
{
    class AgentRuntimeConfigTests
    {
        TEST_CLASS(AgentRuntimeConfigTests);

        TEST_METHOD(MarkdownDefaultsOn)
        {
            VERIFY_IS_TRUE(Config::Snapshot{}.renderAgentMarkdown);
        }

        TEST_METHOD(UnchangedConfigProducesNoEvent)
        {
            Config::Snapshot config;
            VERIFY_IS_TRUE(Config::BuildDeltaPayload(config, config, "window-a").empty());

            config.renderAgentMarkdown = false;
            VERIFY_IS_TRUE(Config::BuildDeltaPayload(config, config, "window-a").empty());
        }

        TEST_METHOD(MarkdownTogglesOnlyEmitMarkdownAndWindow)
        {
            Config::Snapshot previous;
            for (const bool enabled : { false, true, false })
            {
                auto current = previous;
                current.renderAgentMarkdown = enabled;
                const auto payload = Config::BuildDeltaPayload(previous, current, "window-a");

                Json::Value expected{ Json::objectValue };
                expected["window_id"] = "window-a";
                expected["render_agent_markdown"] = enabled;
                VERIFY_IS_TRUE(payload == expected);
                previous = current;
            }
        }

        TEST_METHOD(UnrelatedDeltaOmitsMarkdown)
        {
            Config::Snapshot previous;
            previous.renderAgentMarkdown = false;
            auto current = previous;
            current.autofixEnabled = true;

            Json::Value expected{ Json::objectValue };
            expected["window_id"] = "window-b";
            expected["autofix_enabled"] = true;
            VERIFY_IS_TRUE(Config::BuildDeltaPayload(previous, current, "window-b") == expected);
        }

        TEST_METHOD(YoloOnlyDeltaRemainsTabScoped)
        {
            Config::Snapshot previous;
            auto current = previous;
            current.defaultAgentId = L"codex";
            current.yoloEnabled = true;
            current.yoloPolicyBlocked = true;
            VERIFY_IS_TRUE(Config::BuildDeltaPayload(previous, current, "window-a").empty());
        }

        TEST_METHOD(ReadyPayloadPreservesAutomaticYoloTarget)
        {
            Config::Snapshot current;
            current.yoloEnabled = true;
            const auto payload = Config::BuildReadyPayload("tab-a", "window-a", current);
            VERIFY_IS_TRUE(payload["automatic_yolo_target"].isBool());
            VERIFY_IS_TRUE(payload["automatic_yolo_target"].asBool());
            VERIFY_IS_TRUE(payload["render_agent_markdown"].asBool());
        }

        TEST_METHOD(CombinedDeltaPreservesOtherRuntimeFields)
        {
            Config::Snapshot previous;
            auto current = previous;
            current.delegateAgent = L"claude";
            current.delegateModel = L"model";
            current.customModelSelection = L"custom:provider:model";
            current.autofixEnabled = true;
            current.yoloEnabled = true;
            current.yoloPolicyBlocked = true;
            current.renderAgentMarkdown = false;

            Json::Value expected{ Json::objectValue };
            expected["window_id"] = "window-a";
            expected["delegate_agent"] = "claude";
            expected["delegate_model"] = "model";
            expected["custom_model_selection"] = "custom:provider:model";
            expected["custom_models"] = Json::Value{ Json::arrayValue };
            expected["autofix_enabled"] = true;
            expected["render_agent_markdown"] = false;
            VERIFY_IS_TRUE(Config::BuildDeltaPayload(previous, current, "window-a") == expected);
        }

        TEST_METHOD(ReadyPayloadResendsCurrentMarkdownWithOwner)
        {
            Config::Snapshot current;
            current.yoloPolicyBlocked = true;
            for (const bool enabled : { true, false, true })
            {
                current.renderAgentMarkdown = enabled;
                Json::Value expected{ Json::objectValue };
                expected["tab_id"] = "tab-a";
                expected["window_id"] = "window-a";
                expected["automatic_yolo_target"] = false;
                expected["yolo_enabled"] = false;
                expected["yolo_policy_blocked"] = true;
                expected["render_agent_markdown"] = enabled;
                VERIFY_IS_TRUE(Config::BuildReadyPayload("tab-a", "window-a", current) == expected);
            }
        }

        TEST_METHOD(EnabledHelperArgumentsAreUnchanged)
        {
            std::wstring command{ L"wta --connect-master pipe --owner-tab-id tab-a" };
            const auto original = command;
            Config::AppendHelperArguments(command, true);
            VERIFY_ARE_EQUAL(original, command);
        }

        TEST_METHOD(DisabledHelperArgumentsPreserveIdentity)
        {
            std::wstring command{ L"wta --connect-master pipe --owner-tab-id tab-a" };
            Config::AppendHelperArguments(command, false);
            VERIFY_ARE_EQUAL(
                std::wstring{ L"wta --connect-master pipe --owner-tab-id tab-a --no-agent-markdown" },
                command);
        }
    };
}
