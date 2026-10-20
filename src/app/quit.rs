// SPDX-License-Identifier: MIT

//! Quitting: asking while desktops are open, then ending every session.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// Quitting: asked, confirmed, cancelled, done.
    pub(super) fn quit_message(&mut self, message: QuitMessage) -> Task<cosmic::Action<Message>> {
        match (message, self.quit) {
            (QuitMessage::Requested, Quit::No) => return self.request_quit(false),
            (QuitMessage::Chosen, Quit::No) => return self.request_quit(true),
            (QuitMessage::ToBackground, Quit::Asking) => {
                self.quit = Quit::No;
                return self.hide_window();
            }
            (QuitMessage::Cancel, Quit::Asking) => self.quit = Quit::No,
            (QuitMessage::Confirm, Quit::Asking) => {
                self.quit = Quit::Stopping;
                return self.continue_quit();
            }
            (QuitMessage::ReadyToQuit, _) => return cosmic::iced::exit(),
            _ => {}
        }
        Task::none()
    }

    /// Closing the window (`chosen` false) or choosing Quit: hide, quit, or
    /// ask first (`decide::quit_step`).
    fn request_quit(&mut self, chosen: bool) -> Task<cosmic::Action<Message>> {
        let step = decide::quit_step(
            self.desks.open_count() == 0,
            self.can_run_in_background(),
            chosen,
        );
        match step {
            decide::QuitStep::Hide => self.hide_window(),
            decide::QuitStep::Now => {
                self.quit = Quit::Stopping;
                self.continue_quit()
            }
            decide::QuitStep::Ask => {
                self.quit = Quit::Asking;
                Task::none()
            }
        }
    }

    /// Quitting: once nothing is starting any more, ends every session and
    /// sign-in, waits for FreeRDP to be gone, then exits.
    pub(super) fn continue_quit(&mut self) -> Task<cosmic::Action<Message>> {
        if self.desks.all_launching().next().is_some() || !self.signing_out.is_empty() {
            // `launched` or the end of the sign-out calls this again.
            return Task::none();
        }
        self.desks.stop_all_movers();
        if let Some(Adding::SigningIn { cancel, .. }) = &self.adding {
            cancel.cancel();
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
            off_ui(
                move || {
                    launcher::stop_all(&sessions, launcher::STOP_GRACE);
                    launcher::clean_stale_runtime_dirs();
                },
                (),
            )
            .await;
            Message::Quit(QuitMessage::ReadyToQuit)
        })
    }

    /// Asks before quitting while desktops are open.
    pub(super) fn quit_dialog(&self) -> Element<'_, Message> {
        let open = u32::try_from(self.desks.open_count()).unwrap_or(u32::MAX);
        let mut dialog = widget::dialog()
            .title(fl!("quit-title"))
            .body(fl!("quit-body", count = open))
            .primary_action(
                widget::button::destructive(fl!("quit-disconnect"))
                    .on_press(Message::Quit(QuitMessage::Confirm)),
            )
            .secondary_action(
                widget::button::standard(fl!("cancel"))
                    .on_press(Message::Quit(QuitMessage::Cancel)),
            );
        // Spec §5: the third choice when background operation is on.
        if self.can_run_in_background() {
            dialog = dialog.tertiary_action(
                widget::button::text(fl!("quit-to-background"))
                    .on_press(Message::Quit(QuitMessage::ToBackground)),
            );
        }
        dialog.into()
    }
}
