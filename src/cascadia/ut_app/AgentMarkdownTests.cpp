// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "precomp.h"

#include "../TerminalSettingsModel/GlobalAppSettings.h"

using namespace WEX::TestExecution;
using namespace WEX::Common;
using namespace winrt::Microsoft::Terminal::Settings::Model::implementation;

namespace TerminalAppUnitTests
{
    class AgentMarkdownTests
    {
        TEST_CLASS(AgentMarkdownTests);

        TEST_METHOD(AbsentSettingDefaultsOn)
        {
            const auto settings = GlobalAppSettings::FromJson(Json::Value{ Json::objectValue });
            VERIFY_IS_TRUE(settings->RenderAgentMarkdown());
            VERIFY_IS_FALSE(settings->HasRenderAgentMarkdown());
            VERIFY_IS_FALSE(settings->ToJson().isMember("renderAgentMarkdown"));
        }

        TEST_METHOD(ExplicitFalseRoundTrips)
        {
            Json::Value json{ Json::objectValue };
            json["renderAgentMarkdown"] = false;
            const auto settings = GlobalAppSettings::FromJson(json);
            const auto serialized = settings->ToJson();

            VERIFY_IS_TRUE(serialized.isMember("renderAgentMarkdown"));
            VERIFY_IS_TRUE(serialized["renderAgentMarkdown"].isBool());
            VERIFY_IS_FALSE(serialized["renderAgentMarkdown"].asBool());

            const auto reloaded = GlobalAppSettings::FromJson(serialized);
            VERIFY_ARE_EQUAL(serialized["renderAgentMarkdown"], reloaded->ToJson()["renderAgentMarkdown"]);
            VERIFY_IS_FALSE(reloaded->RenderAgentMarkdown());
        }

        TEST_METHOD(CopyAndOmittedLayerPreserveFalse)
        {
            Json::Value json{ Json::objectValue };
            json["renderAgentMarkdown"] = false;
            const auto settings = GlobalAppSettings::FromJson(json)->Copy();
            settings->LayerJson(Json::Value{ Json::objectValue }, winrt::Microsoft::Terminal::Settings::Model::OriginTag::None);
            VERIFY_IS_TRUE(settings->HasRenderAgentMarkdown());
            VERIFY_IS_FALSE(settings->RenderAgentMarkdown());

            settings->ClearRenderAgentMarkdown();
            VERIFY_IS_TRUE(settings->RenderAgentMarkdown());
            VERIFY_IS_FALSE(settings->ToJson().isMember("renderAgentMarkdown"));
        }

        TEST_METHOD(ProjectedToggleRoundTrips)
        {
            const auto settings = GlobalAppSettings::FromJson(Json::Value{ Json::objectValue });
            winrt::Microsoft::Terminal::Settings::Model::GlobalAppSettings projected = *settings;
            for (const bool enabled : { false, true, false })
            {
                projected.RenderAgentMarkdown(enabled);
                VERIFY_ARE_EQUAL(enabled, projected.RenderAgentMarkdown());
                const auto serialized = settings->ToJson();
                VERIFY_IS_TRUE(serialized["renderAgentMarkdown"].isBool());
                VERIFY_ARE_EQUAL(enabled, serialized["renderAgentMarkdown"].asBool());
                VERIFY_ARE_EQUAL(enabled, GlobalAppSettings::FromJson(serialized)->RenderAgentMarkdown());
            }
        }
    };
}
