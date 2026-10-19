// SPDX-License-Identifier: MIT

//! Provides localization support for this crate.

use i18n_embed::{
    DefaultLocalizer, LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
    unic_langid::LanguageIdentifier,
};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

/// Applies the requested language(s) to requested translations from the `fl!()` macro.
pub fn init(requested_languages: &[LanguageIdentifier]) {
    if let Err(why) = localizer().select(requested_languages) {
        eprintln!("error while loading fluent localizations: {why}");
    }
}

// Get the `Localizer` to be used for localizing this library.
#[must_use]
pub fn localizer() -> Box<dyn Localizer> {
    Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();

    loader
        .load_fallback_language(&Localizations)
        .expect("Error while loading fallback language");

    loader
});

/// Request a localized string by ID from the i18n/ directory.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};

    // The arguments (`name = value, …`) pass through as plain tokens. As
    // `$args:expr` (the COSMIC template's form), rust-analyzer handed each
    // `name = value` on as one assignment expression, so Zed and other
    // editors showed false errors on every call with arguments ("expected
    // HashMap, found ()"); the compiler was fine either way.
    ($message_id:literal, $($args:tt)*) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args)*)
    }};
}
