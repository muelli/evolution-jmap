// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which server a JMAP store talks to, read off its Camel settings.
//!
//! [`crate::settings`] built the object an account's server lives on; this is
//! the other end of it, and the last thing `connect_sync` needs before it can
//! build a client. It is the Camel-side sibling of `jmap-backend-core`'s
//! [`SourceConfig`], not a caller of it: EDS keeps host, port, user and "is it
//! secure" in `ESource` extensions, Camel keeps them on the
//! `CamelNetworkSettings` interface, so the two read different fields. What
//! they must not do is reach different *answers*, so the host validation and
//! the refusal to speak plaintext to anything but loopback are
//! [`jmap_backend_core::source::origin`], shared, and this module is the part
//! that is genuinely Camel's:
//!
//! - **"Not configured" is the empty string.** The `CamelNetworkSettings`
//!   properties are `G_PARAM_CONSTRUCT`, so a settings object nobody touched
//!   has a host of `""` where an unset `ESource` key reads back NULL. Both
//!   spellings mean "no server", and `read_string` already folds them
//!   together — an unconditional read would make an unconfigured account a
//!   request to `https://`.
//! - **The security method is an enum, not a boolean.** Its three values are
//!   names about a protocol JMAP does not have: JMAP is HTTP, so there is
//!   neither a STARTTLS handshake nor an alternate port, and the only bit
//!   really in that field is `NONE` or not.
//! - **The host is punycoded before it is checked.** Camel offers
//!   `dup_host_ensure_ascii` because a host typed into an account editor may
//!   be internationalised while the one that goes on the wire may not, and
//!   the validator deliberately accepts ASCII only. Converting first is what
//!   keeps a perfectly good account from being rejected; converting *before*
//!   validating rather than after is what keeps the checked string and the
//!   sent string the same one.
//!
//! [`SourceConfig`]: jmap_backend_core::source::SourceConfig
//!
//! The password is not here, exactly as it is not in `SourceConfig`. Camel
//! fetches it through the `CamelSession` at connect time; a JMAP account must
//! never take a credential from a settings object that Evolution serialises
//! into a config file.

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CamelNetworkSettings, CamelSettings,
    camel_network_settings_dup_host_ensure_ascii, camel_network_settings_dup_user,
    camel_network_settings_get_port, camel_network_settings_get_security_method,
    camel_network_settings_get_type,
};
use glib_sys::{g_free, gchar, gpointer};
use gobject_sys::{GTypeInstance, g_type_check_instance_is_a};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{SourceError, origin};

/// What a JMAP store needs from its settings in order to build a client.
///
/// The mail counterpart of [`SourceConfig`], minus its `resource_id`: a store
/// is the whole account rather than one collection in it, so there is no
/// server-side object for it to stand for.
///
/// [`SourceConfig`]: jmap_backend_core::source::SourceConfig
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Scheme, host and — if the account names one — port, with no trailing
    /// slash: what `jmap_client::Client::connect` calls the origin.
    pub origin: String,
    /// The user name to authenticate as, if the account names one.
    pub user: Option<String>,
}

impl ServerConfig {
    /// Reads the server out of a store's settings.
    ///
    /// # Safety
    ///
    /// `settings` must be a valid `CamelSettings`, i.e. the one
    /// `camel_service_ref_settings` returned. It is only read from, and
    /// nothing outlives the call.
    pub unsafe fn from_settings(settings: *mut CamelSettings) -> Result<Self, SourceError> {
        // Camel will only hand a service settings of the class its
        // `settings_type` names, and that class is `CamelJmapSettings`, which
        // implements the interface. The check is here anyway because the
        // alternative to it is not a wrong answer, it is a `g_return_if_fail`
        // in each of the four accessors below — four criticals and four NULLs,
        // in a process full of other people's mail. This way the answer is the
        // same one those NULLs would produce, arrived at without asserting.
        if settings.is_null()
            // SAFETY: a non-NULL CamelSettings is a GTypeInstance by the
            // contract above, and the interface type initialises itself.
            || unsafe {
                g_type_check_instance_is_a(
                    settings.cast::<GTypeInstance>(),
                    camel_network_settings_get_type(),
                )
            } == glib_sys::GFALSE
        {
            return Err(SourceError::MissingHost);
        }
        let network = settings.cast::<CamelNetworkSettings>();

        // The host in the form it goes on the wire in. Camel hands back the
        // configured spelling unchanged when it cannot convert one — it does
        // not fail — so a host that is not convertible arrives here still
        // holding whatever the account said, and is rejected by the same
        // validator as any other string that is not a host name. Which is why
        // reading only this spelling is safe: the string that is checked and
        // the string that is used are the same one.
        // SAFETY: `network` implements the interface, checked above; the
        // `dup_` accessor returns a g_malloc'd copy this call frees, rather
        // than a pointer into storage another thread may replace.
        let host = unsafe { take_string(camel_network_settings_dup_host_ensure_ascii(network)) };

        // SAFETY: as above.
        let user = unsafe { take_string(camel_network_settings_dup_user(network)) };
        let port = unsafe { camel_network_settings_get_port(network) };
        let secure = unsafe { camel_network_settings_get_security_method(network) }
            != CAMEL_NETWORK_SECURITY_METHOD_NONE;

        Ok(Self {
            origin: origin(host.as_deref(), port, secure)?,
            user,
        })
    }
}

/// A string a `dup_` accessor just handed over, as an owned `Option<String>`.
///
/// The ownership half of [`read_string`]: same normalisation of "" to absent,
/// and the copy is freed here rather than leaked.
///
/// # Safety
///
/// `s` must be NULL or a NUL-terminated string this call may `g_free`.
unsafe fn take_string(s: *mut gchar) -> Option<String> {
    // SAFETY: the contract above is exactly `read_string`'s, plus ownership.
    let value = unsafe { read_string(s) };
    if !s.is_null() {
        // SAFETY: `s` came from a Camel `dup_` accessor, i.e. from g_malloc,
        // and `read_string` copied what it needed.
        unsafe { g_free(s.cast::<gpointer>() as gpointer) };
    }
    value
}
