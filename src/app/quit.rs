// SPDX-License-Identifier: MIT

//! Quitting: asking while desktops are open, then ending every session.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// Quitting: asked, confirmed, cancelled, done.
    pub(super) fn quit_message(&mut self, message: &Message) -> Task<cosmic::Action<Message>> {
        match (message, self.quit) {
            (Message::QuitChosen, Quit::No) => {
                self.quit_chosen = true;
                return self.quit_message(&Message::QuitRequested);
            }
            (Message::QuitRequested, Quit::No) => {
                let step = decide::quit_step(
                    self.desks.open_count() == 0,
                    self.can_run_in_background(),
                    std::mem::take(&mut self.quit_chosen),
                );
                match step {
                    decide::QuitStep::Hide => return self.hide_window(),
                    decide::QuitStep::Now => {
                        self.quit = Quit::Stopping;
                        return self.continue_quit();
                    }
                    decide::QuitStep::Ask => self.quit = Quit::Asking,
                }
            }
            (Message::QuitToBackground, Quit::Asking) => {
                self.quit = Quit::No;
                return self.hide_window();
            }
            (Message::QuitCancel, Quit::Asking) => self.quit = Quit::No,
            (Message::QuitConfirm, Quit::Asking) => {
                self.quit = Quit::Stopping;
                return self.continue_quit();
            }
            (Message::ReadyToQuit, _) => return cosmic::iced::exit(),
            _ => {}
        }
        Task::none()
    }

    /// Quitting: once nothing is starting any more, ends every session and
    /// sign-in, waits for FreeRDP to be gone, then exits.
    pub(super) fn continue_quit(&mut self) -> Task<cosmic::Action<Message>> {
        if self.desks.all_launching().next().is_some() || !self.signing_out.is_empty() {
            // `launched` or the end of the sign-out calls this again.
            return Task::none();
        }
        self.desks.stop_all_movers();
        if let Some(Adding {
            cancel: Some(c), ..
        }) = &self.adding
        {
            c.cancel();
        }
        for a in &self.accounts {
            if let AccountState::SigningIn { cancel } = &a.state {
                cancel.cancel();
            }
        }
        let sessions = self.desks.take_all_running();
        if !sessions.is_empty() {
            crate::app_log!("quitting: ending {} session(s)", sessions.len());
        }
        cosmic::task::future(async move {
            let _ = tokio::task::spawn_blocking(move || {
                launcher::stop_all(&sessions, launcher::STOP_GRACE);
                launcher::clean_stale_runtime_dirs();
            })
            .await;
            Message::ReadyToQuit
        })
    }

    /// Asks before quitting while desktops are open.
    pub(super) fn quit_dialog(&self) -> Element<'_, Message> {
        let open = u32::try_from(self.desks.open_count()).unwrap_or(u32::MAX);
        let mut dialog = widget::dialog()
            .title(fl!("quit-title"))
            .body(fl!("quit-body", count = open))
            .primary_action(
                widget::button::destructive(fl!("quit-disconnect")).on_press(Message::QuitConfirm),
            )
            .secondary_action(
                widget::button::standard(fl!("cancel")).on_press(Message::QuitCancel),
            );
        // Spec §5: the third choice when background operation is on.
        if self.can_run_in_background() {
            dialog = dialog.tertiary_action(
                widget::button::text(fl!("quit-to-background")).on_press(Message::QuitToBackground),
            );
        }
        dialog.into()
    }
}
