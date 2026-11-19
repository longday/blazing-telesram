# Third-party notices

- `crates/desktop/src/input.rs` adapts the [GPUI 0.2.2 input example](https://docs.rs/crate/gpui/0.2.2/source/examples/input.rs) from Zed Industries, Inc.; the copyright and license are preserved in [licenses/GPUI-APACHE-2.0](licenses/GPUI-APACHE-2.0). Changes include the reusable text-input entity, styling, selection and single-line behavior used in the demo. This file remains under Apache-2.0.
- `vendor/xattr` is [xattr 0.2.3](https://crates.io/crates/xattr/0.2.3), offered under MIT OR Apache-2.0. Both upstream licenses are included. The vendored copy changes Linux/Android to use `ENODATA` for a missing extended attribute, avoiding a removed libc alias.

The app uses GPUI from crates.io. No Telegram or Yandex client code, logos, or assets are included.
