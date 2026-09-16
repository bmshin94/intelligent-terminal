// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::EMBEDDED_LOCALES;

pub(crate) struct Embedded;

impl rust_i18n::Backend for Embedded {
    fn available_locales(&self) -> Vec<&str> {
        EMBEDDED_LOCALES.iter().map(|(locale, _)| *locale).collect()
    }

    fn translate(&self, locale: &str, key: &str) -> Option<&str> {
        let locale_index = EMBEDDED_LOCALES
            .binary_search_by(|(candidate, _)| candidate.cmp(&locale))
            .ok()?;
        let entries = EMBEDDED_LOCALES[locale_index].1;
        let key_index = entries
            .binary_search_by(|(candidate, _)| candidate.cmp(&key))
            .ok()?;
        Some(entries[key_index].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_i18n::Backend;

    #[test]
    fn complete_catalogue_and_first_macro_lookup_fit_a_small_thread_stack() {
        let _locale = crate::test_support::lock_locale();
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(|| {
                let backend = Embedded;
                let locales = backend.available_locales();
                assert!(locales.len() >= 89);
                assert!(locales.windows(2).all(|pair| pair[0] < pair[1]));
                for (locale, entries) in EMBEDDED_LOCALES {
                    assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));
                    for (key, value) in *entries {
                        assert_eq!(backend.translate(locale, key), Some(*value));
                    }
                }
                assert_ne!(
                    backend.translate("en-US", "agent_center.status_ok"),
                    backend.translate("zh-CN", "agent_center.status_ok"),
                );
                assert_eq!(t!("agent_center.title", locale = "zz-ZZ"), "Agent Center");
                assert!(t!(
                    "agent_center.missing_field",
                    locale = "en-US",
                    field = "workId"
                )
                .contains("workId"));
                assert!(backend
                    .translate("en-US", "nonexistent.translation.key")
                    .is_none());
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
