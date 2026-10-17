// SPDX-License-Identifier: MIT

//! Optional Windows account overrides for host pools that ask for a
//! password (no Entra single sign-on). See
//! docs/adr/0005-password-login-host-pools.md.
//!
//! By default we pass no user name or domain at all: `FreeRDP` then uses the
//! user name the AVD service returns and the `AzureAD` domain. This module
//! validates what a user types into the per-resource override, in any
//! supported form, and converts it to `FreeRDP`'s user and domain fields.

use std::fmt;

/// Domain prefix Windows uses for Microsoft Entra accounts.
const AZURE_AD: &str = "AzureAD";

/// Upper bounds well above real Windows limits, to reject garbage early.
const MAX_UPN_LEN: usize = 256;
const MAX_DOMAIN_LEN: usize = 255;
const MAX_NAME_LEN: usize = 256;

/// Characters Windows does not allow in logon names. `\` and `@` are
/// handled separately as separators.
const FORBIDDEN: &[char] = &[
    '"', '/', '[', ']', ':', ';', '|', '=', ',', '+', '*', '?', '<', '>',
];

/// A parsed Windows account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    /// `user@domain`, an Entra or Active Directory user principal name.
    Upn(String),
    /// `DOMAIN\name`, an Active Directory account.
    DomainAccount { domain: String, name: String },
}

/// How a UPN account is sent to the session host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogonForm {
    /// `user@domain` with an empty domain.
    Upn,
    /// `AzureAD\user@domain`, for Entra-joined session hosts.
    AzureAdUpn,
}

/// What a user typed, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUserName {
    pub account: Account,
    /// The form the user typed explicitly, if any. `None` for a plain UPN,
    /// where the caller picks the form (the remembered or default one).
    pub explicit_form: Option<LogonForm>,
}

/// The user and domain fields passed to `FreeRDP` (`/u:` and `/d:`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeRdpCredentials {
    pub user: String,
    /// Empty for a plain UPN, as `FreeRDP` expects.
    pub domain: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserNameError {
    Empty,
    /// Spaces, tabs, line breaks or other control characters.
    InvalidWhitespaceOrControl,
    /// A character Windows does not allow in logon names.
    InvalidCharacter(char),
    /// More than one `\` or more than one `@`.
    TooManySeparators,
    /// Something like `@domain`, `user@`, `\name` or `DOMAIN\`.
    MissingPart,
    /// `AzureAD\name` without `@domain`.
    AzureAdNeedsUpn,
    /// `.\name` (local account) is not supported for AVD.
    LocalAccountNotSupported,
    TooLong,
}

impl fmt::Display for UserNameError {
    // Developer-facing text. The dialog shows localized messages per variant.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "user name is empty"),
            Self::InvalidWhitespaceOrControl => {
                write!(f, "user name contains spaces or control characters")
            }
            Self::InvalidCharacter(c) => write!(f, "user name contains '{c}'"),
            Self::TooManySeparators => write!(f, "user name has more than one '\\' or '@'"),
            Self::MissingPart => write!(f, "user name is incomplete"),
            Self::AzureAdNeedsUpn => write!(f, "AzureAD\\ must be followed by user@domain"),
            Self::LocalAccountNotSupported => {
                write!(f, "local accounts (.\\name) are not supported")
            }
            Self::TooLong => write!(f, "user name is too long"),
        }
    }
}

impl std::error::Error for UserNameError {}

/// Parses `user@domain`, `AzureAD\user@domain` or `DOMAIN\name`.
/// Surrounding whitespace is ignored; anything else is an error.
pub fn parse(input: &str) -> Result<ParsedUserName, UserNameError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(UserNameError::Empty);
    }
    if input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(UserNameError::InvalidWhitespaceOrControl);
    }
    if let Some(c) = input.chars().find(|c| FORBIDDEN.contains(c)) {
        return Err(UserNameError::InvalidCharacter(c));
    }
    if input.matches('\\').count() > 1 || input.matches('@').count() > 1 {
        return Err(UserNameError::TooManySeparators);
    }

    match input.split_once('\\') {
        None => Ok(ParsedUserName {
            account: Account::Upn(parse_upn(input)?),
            explicit_form: None,
        }),
        Some((domain, rest)) => {
            if domain.is_empty() || rest.is_empty() {
                return Err(UserNameError::MissingPart);
            }
            if domain == "." {
                return Err(UserNameError::LocalAccountNotSupported);
            }
            if domain.eq_ignore_ascii_case(AZURE_AD) {
                if !rest.contains('@') {
                    return Err(UserNameError::AzureAdNeedsUpn);
                }
                return Ok(ParsedUserName {
                    account: Account::Upn(parse_upn(rest)?),
                    explicit_form: Some(LogonForm::AzureAdUpn),
                });
            }
            if rest.contains('@') {
                // `DOMAIN\user@domain` is ambiguous; ask for one form.
                return Err(UserNameError::TooManySeparators);
            }
            if domain.len() > MAX_DOMAIN_LEN || rest.len() > MAX_NAME_LEN {
                return Err(UserNameError::TooLong);
            }
            Ok(ParsedUserName {
                account: Account::DomainAccount {
                    domain: domain.to_owned(),
                    name: rest.to_owned(),
                },
                explicit_form: None,
            })
        }
    }
}

fn parse_upn(s: &str) -> Result<String, UserNameError> {
    let (local, domain) = s.split_once('@').ok_or(UserNameError::MissingPart)?;
    if local.is_empty() || domain.is_empty() {
        return Err(UserNameError::MissingPart);
    }
    if s.len() > MAX_UPN_LEN {
        return Err(UserNameError::TooLong);
    }
    Ok(s.to_owned())
}

impl Account {
    /// The fields `FreeRDP` receives. `form` only matters for UPN accounts.
    pub fn freerdp_credentials(&self, form: LogonForm) -> FreeRdpCredentials {
        match (self, form) {
            (Self::Upn(upn), LogonForm::Upn) => FreeRdpCredentials {
                user: upn.clone(),
                domain: String::new(),
            },
            (Self::Upn(upn), LogonForm::AzureAdUpn) => FreeRdpCredentials {
                user: upn.clone(),
                domain: AZURE_AD.to_owned(),
            },
            (Self::DomainAccount { domain, name }, _) => FreeRdpCredentials {
                user: name.clone(),
                domain: domain.clone(),
            },
        }
    }

    /// The user name exactly as it will be sent, for showing in the dialog
    /// and in the "wrong user name or password" error.
    pub fn display(&self, form: LogonForm) -> String {
        let c = self.freerdp_credentials(form);
        if c.domain.is_empty() {
            c.user
        } else {
            format!("{}\\{}", c.domain, c.user)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upn(s: &str) -> Account {
        Account::Upn(s.to_owned())
    }

    #[test]
    fn plain_upn_leaves_form_to_caller() {
        let p = parse("  anna@example.com ").unwrap();
        assert_eq!(p.account, upn("anna@example.com"));
        assert_eq!(p.explicit_form, None);
    }

    #[test]
    fn azure_ad_prefix_is_case_insensitive_and_sets_form() {
        for input in ["AzureAD\\anna@example.com", "azuread\\anna@example.com"] {
            let p = parse(input).unwrap();
            assert_eq!(p.account, upn("anna@example.com"));
            assert_eq!(p.explicit_form, Some(LogonForm::AzureAdUpn));
        }
    }

    #[test]
    fn domain_account() {
        let p = parse("CORP\\anna").unwrap();
        assert_eq!(
            p.account,
            Account::DomainAccount {
                domain: "CORP".into(),
                name: "anna".into()
            }
        );
    }

    #[test]
    fn freerdp_fields_match_freerdp_parsing() {
        // FreeRDP keeps `user@domain` whole with an empty domain, and splits
        // `DOMAIN\user` at the backslash (client/common/cmdline.c).
        let a = upn("anna@example.com");
        assert_eq!(
            a.freerdp_credentials(LogonForm::Upn),
            FreeRdpCredentials {
                user: "anna@example.com".into(),
                domain: String::new()
            }
        );
        assert_eq!(
            a.freerdp_credentials(LogonForm::AzureAdUpn),
            FreeRdpCredentials {
                user: "anna@example.com".into(),
                domain: "AzureAD".into()
            }
        );
        let d = parse("CORP\\anna").unwrap().account;
        assert_eq!(
            d.freerdp_credentials(LogonForm::AzureAdUpn),
            FreeRdpCredentials {
                user: "anna".into(),
                domain: "CORP".into()
            }
        );
    }

    #[test]
    fn display_shows_what_is_sent() {
        let a = upn("anna@example.com");
        assert_eq!(a.display(LogonForm::Upn), "anna@example.com");
        assert_eq!(
            a.display(LogonForm::AzureAdUpn),
            "AzureAD\\anna@example.com"
        );
    }

    #[test]
    fn same_account_whichever_form_was_typed() {
        let typed_upn = parse("anna@example.com").unwrap().account;
        let typed_azure = parse("AzureAD\\anna@example.com").unwrap().account;
        assert_eq!(typed_upn, typed_azure);
    }

    #[test]
    fn rejects_malformed_input() {
        use UserNameError::*;
        let cases = [
            ("", Empty),
            ("   ", Empty),
            ("anna", MissingPart),
            ("@example.com", MissingPart),
            ("anna@", MissingPart),
            ("\\anna", MissingPart),
            ("CORP\\", MissingPart),
            ("AzureAD\\", MissingPart),
            ("AzureAD\\anna", AzureAdNeedsUpn),
            (".\\anna", LocalAccountNotSupported),
            ("a\\b\\c", TooManySeparators),
            ("a@b@c", TooManySeparators),
            ("CORP\\anna@example.com", TooManySeparators),
            ("anna smith@example.com", InvalidWhitespaceOrControl),
            ("anna@example.com\n/p:x", InvalidWhitespaceOrControl),
            ("anna\0@example.com", InvalidWhitespaceOrControl),
            ("anna@exa\u{7f}mple.com", InvalidWhitespaceOrControl),
            ("an\"na@example.com", InvalidCharacter('"')),
            ("CORP\\an:na", InvalidCharacter(':')),
            ("/u:anna@example.com", InvalidCharacter('/')),
        ];
        for (input, want) in cases {
            assert_eq!(parse(input), Err(want), "input {input:?}");
        }
    }

    #[test]
    fn rejects_over_long_values() {
        let long_upn = format!("{}@example.com", "a".repeat(MAX_UPN_LEN));
        assert_eq!(parse(&long_upn), Err(UserNameError::TooLong));
        let long_domain = format!("{}\\anna", "D".repeat(MAX_DOMAIN_LEN + 1));
        assert_eq!(parse(&long_domain), Err(UserNameError::TooLong));
    }
}
