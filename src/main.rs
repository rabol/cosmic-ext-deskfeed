// SPDX-License-Identifier: MIT

mod accounts;
mod app;
mod auth;
mod config;
mod deps;
mod i18n;
mod log;
mod secrets;
mod service;
mod sessions;
mod settings;
#[cfg(test)]
mod test_util;
mod workspace;

fn main() -> cosmic::iced::Result {
    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    // Settings for configuring the application window and iced runtime.
    let settings = cosmic::app::Settings::default().size_limits(
        cosmic::iced::Limits::NONE
            .min_width(360.0)
            .min_height(180.0),
    );

    // Starts the application's event loop with `()` as the application's flags.
    cosmic::app::run::<app::AppModel>(settings, ())
}
