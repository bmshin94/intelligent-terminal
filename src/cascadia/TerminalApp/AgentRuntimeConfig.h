// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#pragma once

#include "../inc/CustomModelProviderUtils.h"

namespace TerminalApp::AgentRuntimeConfig
{
    struct Snapshot
    {
        std::wstring delegateAgent;
        std::wstring delegateModel;
        std::wstring customModelSelection;
        std::vector<::Microsoft::Terminal::CustomModels::CatalogEntry> customModels;
        bool autofixEnabled{ false };
        std::wstring defaultAgentId;
        bool yoloEnabled{ false };
        bool yoloPolicyBlocked{ false };
        bool renderAgentMarkdown{ true };
    };

    // Automatic approval is resolved per tab/provider by TerminalPage, not broadcast here.
    inline Json::Value BuildDeltaPayload(
        const Snapshot& last,
        const Snapshot& current,
        const std::string_view windowId)
    {
        Json::Value params{ Json::objectValue };
        if (last.autofixEnabled != current.autofixEnabled)
        {
            params["autofix_enabled"] = current.autofixEnabled;
        }
        if (last.delegateAgent != current.delegateAgent ||
            last.delegateModel != current.delegateModel)
        {
            params["delegate_agent"] = winrt::to_string(current.delegateAgent);
            params["delegate_model"] = winrt::to_string(current.delegateModel);
        }
        if (last.customModelSelection != current.customModelSelection ||
            last.customModels != current.customModels)
        {
            params["custom_model_selection"] = winrt::to_string(current.customModelSelection);
            params["custom_models"] =
                ::Microsoft::Terminal::CustomModels::CatalogToJson(current.customModels);
        }
        if (last.renderAgentMarkdown != current.renderAgentMarkdown)
        {
            params["render_agent_markdown"] = current.renderAgentMarkdown;
        }
        if (!params.empty())
        {
            params["window_id"] = std::string{ windowId };
        }
        return params;
    }

    inline Json::Value BuildReadyPayload(
        const std::string_view tabId,
        const std::string_view windowId,
        const Snapshot& current)
    {
        Json::Value params{ Json::objectValue };
        params["tab_id"] = std::string{ tabId };
        params["window_id"] = std::string{ windowId };
        params["automatic_yolo_target"] = current.yoloEnabled;
        params["yolo_enabled"] = current.yoloEnabled;
        params["yolo_policy_blocked"] = current.yoloPolicyBlocked;
        params["render_agent_markdown"] = current.renderAgentMarkdown;
        return params;
    }

    inline void AppendHelperArguments(std::wstring& command, const bool renderAgentMarkdown)
    {
        if (!renderAgentMarkdown)
        {
            command.append(L" --no-agent-markdown");
        }
    }
}
