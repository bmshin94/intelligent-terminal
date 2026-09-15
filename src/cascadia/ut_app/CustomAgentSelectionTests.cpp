// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "precomp.h"

#include "../TerminalSettingsEditor/CustomAgentSelection.h"
#include "../TerminalSettingsModel/GlobalAppSettings.h"

using namespace WEX::TestExecution;
using namespace WEX::Common;
using namespace winrt::Windows::Foundation::Collections;
using namespace winrt::Microsoft::Terminal::Settings::Model::implementation;
namespace Selection = ::Microsoft::Terminal::Settings::Editor::CustomAgentSelection;

namespace TerminalAppUnitTests
{
    class CustomAgentSelectionTests
    {
        TEST_CLASS(CustomAgentSelectionTests);

        static constexpr std::wstring_view NamedId{ L"custom:markdown-fixture" };
        static constexpr std::wstring_view LegacyCommand{ L"pwsh -NoProfile -EncodedCommand RQB4AGkAdAA=" };

        static IVector<winrt::hstring> Commands()
        {
            return winrt::single_threaded_vector<winrt::hstring>({ winrt::hstring{ LegacyCommand } });
        }

        static void VerifySelection(const winrt::hstring& selectedId, const winrt::hstring& legacyCommand)
        {
            const auto command = Selection::ResolveCommand(Commands(), selectedId, legacyCommand);
            VERIFY_ARE_EQUAL(legacyCommand, command);
            VERIFY_ARE_EQUAL(selectedId, Selection::EntryId(command, selectedId, command));
        }

        TEST_METHOD(NamedLegacySelectionResolvesItsCommand)
        {
            VERIFY_ARE_EQUAL(winrt::hstring{ LegacyCommand },
                             Selection::ResolveCommand(Commands(), winrt::hstring{ NamedId }, winrt::hstring{ LegacyCommand }));
        }

        TEST_METHOD(NamedLegacyEntryPreservesItsId)
        {
            VERIFY_ARE_EQUAL(winrt::hstring{ NamedId },
                             Selection::EntryId(winrt::hstring{ LegacyCommand }, winrt::hstring{ NamedId }, winrt::hstring{ LegacyCommand }));
        }

        TEST_METHOD(UnrelatedDisplaySavePreservesNamedProviders)
        {
            Json::Value json{ Json::objectValue };
            json["acpAgent"] = winrt::to_string(NamedId);
            json["acpCustomCommand"] = winrt::to_string(LegacyCommand);
            json["acpModel"] = "fixture-model";
            json["delegateAgent"] = "custom:delegate-fixture";
            json["delegateCustomCommand"] = winrt::to_string(LegacyCommand);
            auto settings = GlobalAppSettings::FromJson(json)->Copy();

            for (const bool enabled : { false, true, false })
            {
                VerifySelection(settings->AcpAgent(), settings->AcpCustomCommand());
                VerifySelection(settings->DelegateAgent(), settings->DelegateCustomCommand());
                settings->RenderAgentMarkdown(enabled);

                const auto saved = settings->ToJson();
                VERIFY_IS_TRUE(saved["acpAgent"] == json["acpAgent"]);
                VERIFY_IS_TRUE(saved["acpCustomCommand"] == json["acpCustomCommand"]);
                VERIFY_IS_TRUE(saved["acpModel"] == json["acpModel"]);
                VERIFY_IS_TRUE(saved["delegateAgent"] == json["delegateAgent"]);
                VERIFY_IS_TRUE(saved["delegateCustomCommand"] == json["delegateCustomCommand"]);
                VERIFY_ARE_EQUAL(enabled, saved["renderAgentMarkdown"].asBool());
                settings = GlobalAppSettings::FromJson(saved);
            }
        }

        TEST_METHOD(InheritedNamedSelectionDoesNotRequireLocalOverrides)
        {
            Json::Value json{ Json::objectValue };
            json["acpAgent"] = winrt::to_string(NamedId);
            json["acpCustomCommand"] = winrt::to_string(LegacyCommand);
            const auto settings = GlobalAppSettings::FromJson(Json::Value{ Json::objectValue });
            settings->AddLeastImportantParent(GlobalAppSettings::FromJson(json));
            VerifySelection(settings->AcpAgent(), settings->AcpCustomCommand());
            settings->RenderAgentMarkdown(false);

            const auto saved = settings->ToJson();
            VERIFY_IS_FALSE(saved.isMember("acpAgent"));
            VERIFY_IS_FALSE(saved.isMember("acpCustomCommand"));
            VERIFY_IS_FALSE(saved["renderAgentMarkdown"].asBool());
        }

        TEST_METHOD(RegisteredCommandKeepsPrecedence)
        {
            const auto commands = winrt::single_threaded_vector<winrt::hstring>({ L"pwsh --registered" });
            VERIFY_ARE_EQUAL(winrt::hstring{ L"pwsh --registered" },
                             Selection::ResolveCommand(commands, L"custom:pwsh", winrt::hstring{ LegacyCommand }));
        }

        TEST_METHOD(OtherEntriesKeepTheirDerivedIds)
        {
            VERIFY_ARE_EQUAL(winrt::hstring{ L"custom:other" },
                             Selection::EntryId(L"other.exe --acp", winrt::hstring{ NamedId }, winrt::hstring{ LegacyCommand }));
            VERIFY_ARE_EQUAL(winrt::hstring{ L"custom:pwsh" }, Selection::CommandId(winrt::hstring{ LegacyCommand }));
        }

        TEST_METHOD(InvalidOrMissingLegacyCommandsDoNotCreateASelection)
        {
            for (const auto command : { L"", L" \t ", L"\"\"" })
            {
                VERIFY_IS_TRUE(Selection::ResolveCommand(Commands(), winrt::hstring{ NamedId }, command).empty());
                VERIFY_IS_TRUE(Selection::EntryId(command, winrt::hstring{ NamedId }, command).empty());
            }
            VERIFY_IS_TRUE(Selection::ResolveCommand(Commands(), L"copilot", winrt::hstring{ LegacyCommand }).empty());
            VERIFY_IS_TRUE(Selection::ResolveCommand(Commands(), L"custom:", winrt::hstring{ LegacyCommand }).empty());
        }
    };
}
