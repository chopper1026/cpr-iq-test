# Third-party components

- The plugin uses the Apache-2.0 `gateway-plugin-sdk` from `zyycn/codex-proxy-rs`; the exact SDK commit is pinned in Cargo.toml. The CPR `cpr-plugin` tool packages builds but is not shipped in the plugin archive.
- The plugin links the MIT Rust SDK from `Ikaleio/lm-detector` and redistributes the matching offline reference bank and detector from the same upstream revision. See LICENSE-lm-detector and data/detector.lock.json for attribution and content hashes. References to upstream in this project do not imply endorsement.
- Other linked Rust dependencies and their exact versions are recorded in Cargo.lock.
