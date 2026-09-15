// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#pragma once

#include <winrt/Windows.Foundation.Collections.h>
#include "../inc/CustomAgentId.h"

namespace Microsoft::Terminal::Settings::Editor::CustomAgentSelection
{
    inline bool IsNamedCustomId(const std::wstring_view id) noexcept
    {
        return id.starts_with(L"custom:") && id.size() > 7;
    }

    inline winrt::hstring CommandId(const winrt::hstring& command)
    {
        const auto bareId = Model::DeriveCustomAgentId(std::wstring_view{ command });
        return bareId.empty() ? winrt::hstring{} : winrt::hstring{ L"custom:" } + bareId;
    }

    inline winrt::hstring ResolveCommand(
        const winrt::Windows::Foundation::Collections::IVector<winrt::hstring>& commands,
        const winrt::hstring& selectedId,
        const winrt::hstring& legacyCommand)
    {
        if (commands)
        {
            for (const auto& command : commands)
            {
                const auto id = CommandId(command);
                if (!id.empty() && id == selectedId)
                {
                    return command;
                }
            }
        }
        // Legacy settings pair an arbitrary custom:<name> with a command;
        // the name need not match the command's executable.
        if (IsNamedCustomId(selectedId) && !CommandId(legacyCommand).empty())
        {
            return legacyCommand;
        }
        return {};
    }

    inline winrt::hstring EntryId(
        const winrt::hstring& command,
        const winrt::hstring& selectedId,
        const winrt::hstring& selectedCommand)
    {
        const auto id = CommandId(command);
        return !id.empty() && IsNamedCustomId(selectedId) && command == selectedCommand ? selectedId : id;
    }
}
