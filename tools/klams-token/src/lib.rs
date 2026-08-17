//! `klams-token` — structural editor for the `[[auth.tokens]]` grants
//! in `klams.toml` (sprint 045, korg #265).
//!
//! korg #264 was a live incident: a hand-edit of `/etc/klams/klams.toml`
//! clobbered an existing grant, because nothing understood the file's
//! structure — only text editors did, and the file lives outside any
//! repo behind `sudo`, so there was no diff or review step either.
//!
//! Two properties answer that, and they are the reason this crate
//! exists rather than a shell script:
//!
//! 1. **Structural editing** ([`doc`]) — grants are addressed as TOML
//!    tables, so a write cannot silently overwrite a sibling. Editing is
//!    format-preserving: the live file is heavily commented and those
//!    comments are the operator documentation.
//! 2. **Fingerprint-and-refuse** ([`fingerprint`]) — every write states
//!    the change it intends, and the grant set is fingerprinted before
//!    and after. Anything else moving aborts the write. This is what
//!    makes a clobber *impossible* rather than merely unlikely, and it
//!    is lifted from the ~40-line version k-homelab sprint 016 (S4)
//!    wrote for a one-off grant removal.
//!
//! The schema comes from [`klams_types::AuthConfig`] — the same type
//! `klams-service` boots from. A config editor whose understanding of
//! the schema can drift from the service's is precisely the bug this
//! tool exists to prevent.

pub mod doc;
pub mod fingerprint;
pub mod paths;
pub mod verify;
pub mod writer;

pub use doc::{GrantView, GrantsDoc};
pub use fingerprint::{verify_delta, Change, GrantFingerprint};
