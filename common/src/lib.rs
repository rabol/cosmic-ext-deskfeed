// SPDX-License-Identifier: MIT

//! Code shared by Deskfeed and its sign-in helper, which are separate
//! programs built from one workspace.

pub mod helper;
pub mod lines;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
