// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The first thing a program says on a computer that cannot bound a run.

use super::*;

#[test]
fn linux_is_not_refused() {
    assert_eq!(host_fix("linux"), None);
}

/// The fix is the one for the computer named, and it says where to read more.
#[test]
fn windows_and_macos_are_told_their_own_fix() {
    let windows = host_fix("windows").expect("refused");
    assert!(windows.contains("this is Windows") && windows.contains("wsl --install -d Ubuntu"));
    let macos = host_fix("macos").expect("refused");
    assert!(macos.contains("this is macOS") && macos.contains("Linux virtual machine"));
    for said in [windows, macos, host_fix("freebsd").expect("refused")] {
        assert!(said.contains(INSTALLATION), "{said}");
    }
}
