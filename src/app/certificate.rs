// SPDX-License-Identifier: MIT

//! Session host certificates: the approval dialog and approved
//! fingerprints (ADR 0016).

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    pub(super) fn save_certificate_pins(&mut self, pins: BTreeMap<String, String>) {
        match &self.config_handler {
            Some(handler) => {
                let _ = self.config.set_certificate_pins(handler, pins);
            }
            None => self.config.certificate_pins = pins,
        }
    }
}

/// The certificate dialog: what the session host presented, and either
/// "Trust and connect" or, for a changed certificate, only Close.
pub(super) fn certificate_dialog(d: &CertificateDialog) -> Element<'_, Message> {
    let c = &d.certificate;
    let space = cosmic::theme::spacing();
    let unknown = || fl!("certificate-unknown");
    let or_unknown = |s: &str| {
        if s.is_empty() || s == "(null)" {
            unknown()
        } else {
            s.to_owned()
        }
    };
    let row = |label: String, value: String| {
        widget::settings::item::builder(label).control(widget::text::body(value))
    };
    let details = widget::settings::section()
        .add(row(fl!("certificate-host"), or_unknown(&c.host)))
        .add(row(fl!("certificate-name"), or_unknown(&c.common_name)))
        .add(row(fl!("certificate-issuer"), or_unknown(&c.issuer)))
        .add(row(
            fl!("certificate-valid"),
            fl!(
                "certificate-valid-range",
                from = or_unknown(&c.valid_from),
                to = or_unknown(&c.valid_to)
            ),
        ));
    let mut body = widget::column::with_capacity(5).spacing(space.space_s);
    match &d.changed_from {
        None => {
            body = body.push(widget::text::body(fl!(
                "certificate-untrusted-body",
                title = d.title.clone()
            )));
        }
        Some(_) => {
            body = body.push(widget::text::body(fl!(
                "certificate-changed-body",
                title = d.title.clone()
            )));
        }
    }
    body = body.push(details);
    body = body.push(widget::text::caption(fl!("certificate-fingerprint")));
    body = body.push(widget::text::monotext(c.fingerprint.clone()));
    if let Some(old) = &d.changed_from {
        body = body.push(widget::text::caption(fl!(
            "certificate-approved-fingerprint"
        )));
        body = body.push(widget::text::monotext(old.clone()));
    }
    let mut dialog = widget::dialog().title(match d.changed_from {
        None => fl!("certificate-untrusted-title"),
        Some(_) => fl!("certificate-changed-title"),
    });
    dialog = dialog.control(body);
    if d.changed_from.is_none() {
        dialog = dialog
            .primary_action(
                widget::button::destructive(fl!("certificate-trust"))
                    .on_press(Message::Connection(ConnectionMessage::TrustCertificate)),
            )
            .secondary_action(widget::button::standard(fl!("cancel")).on_press(
                Message::Connection(ConnectionMessage::CloseCertificateDialog),
            ));
    } else {
        dialog = dialog.primary_action(widget::button::standard(fl!("close")).on_press(
            Message::Connection(ConnectionMessage::CloseCertificateDialog),
        ));
    }
    dialog.into()
}
