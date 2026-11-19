# Third-party notices

- `crates/desktop/src/input.rs` adapts the [GPUI 0.2.2 input example](https://docs.rs/crate/gpui/0.2.2/source/examples/input.rs) from Zed Industries, Inc.; the copyright and license are preserved in [licenses/GPUI-APACHE-2.0](licenses/GPUI-APACHE-2.0). Changes include the reusable text-input entity, styling, selection and single-line behavior used in the demo. This file remains under Apache-2.0.
- `vendor/xattr` is [xattr 0.2.3](https://crates.io/crates/xattr/0.2.3), offered under MIT OR Apache-2.0. Both upstream licenses are included. The vendored copy changes Linux/Android to use `ENODATA` for a missing extended attribute, avoiding a removed libc alias.
- `vendor/block` is [block 0.1.6](https://crates.io/crates/block/0.1.6), declared MIT in its manifest; upstream ships no license text. The vendored copy makes the opaque `Class` type a zero-sized struct instead of an uninhabited enum (future hard error, rust-lang/rust#74840) and spells out the default `extern "C"` ABI.
- `vendor/proc-macro-error2` is [proc-macro-error2 2.0.1](https://crates.io/crates/proc-macro-error2/2.0.1), offered under MIT OR Apache-2.0. Both upstream licenses are included. The vendored copy declares `pub extern crate proc_macro` so its re-export stays legal (rust-lang/rust#127909).

The app uses GPUI from crates.io. No Telegram or Yandex client code, logos, or assets are included.
