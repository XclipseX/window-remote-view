//! `librv_core.so`: the Rust client core embedded in the Android app.
//!
//! * [`glue`]: platform-neutral translation (Kotlin input -> `InputEvent`,
//!   `ClientEvent` -> Kotlin callbacks). Unit tested on the host.
//! * [`worker`]: thread that owns `rv_session::ClientSession`.
//! * [`lan`]: LAN UDP session I/O and beacon discovery.
//! * `jni_bridge`: the `Java_com_remoteview_client_NativeBridge_*` symbols and
//!   the callbacks into Kotlin.
//!
//! # Pairing (USB and LAN modes)
//! Host and client derive the Noise PSK from `Argon2id(otp, host_id)`.
//! `host_id` is the fixed constant [`rv_session::USB_HOST_ID`] (no pre-handshake
//! hello exists); the OTP is the 6-9 digit code `rv-host` prints at startup
//! (or fixes via `--otp`). The Android app sends the PIN the user typed as the
//! `"otp"` field of the hello JSON passed to `nativeConnectLink` / `nativeConnect`.

pub mod glue;
mod jni_bridge;
pub mod lan;
pub mod worker;
