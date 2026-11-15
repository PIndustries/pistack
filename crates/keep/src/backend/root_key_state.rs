// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use super::consts::{META_ROOT_KEY, META_ROOT_KEY_WRAPPED};
use super::wrapped_root_key::WrappedRootKey;
use crate::error::Result;

/// What a store holds for the root key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootKeyState {
    /// No key yet: the first server start creates one.
    Empty,
    /// [`META_ROOT_KEY`] only.
    Software,
    /// [`META_ROOT_KEY_WRAPPED`] only.
    Wrapped(WrappedRootKey),
    /// Both: refused at startup until a migrate command settles it.
    Both(WrappedRootKey),
}

pub fn root_key_state(store: &Store) -> Result<RootKeyState> {
    let plain = store.get_meta::<String>(META_ROOT_KEY)?;
    let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
    Ok(match (plain, wrapped) {
        (None, None) => RootKeyState::Empty,
        (Some(_), None) => RootKeyState::Software,
        (None, Some(w)) => RootKeyState::Wrapped(w),
        (Some(_), Some(w)) => RootKeyState::Both(w),
    })
}
