# Test fixtures

`rar4.rar` and `rar_enc.rar` come from the libarchive test suite
(`libarchive/test/test_read_format_rar.rar.uu` and
`test_read_format_rar_encryption_data.rar.uu`, decoded), which is distributed
under the BSD-2-Clause license. `rar_enc.rar` encrypts member data only; its
password is `12345678`, as documented in the matching libarchive test.

They are used by `src-tauri/src/archive/mod.rs` to cover RAR listing,
extraction, symlink skipping and the password flow; no RAR writer exists in
this project (the UnRAR license forbids building one).
