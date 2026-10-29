# Test fixtures

`rar4.rar` and `rar_enc.rar` come from the libarchive test suite
(`libarchive/test/test_read_format_rar.rar.uu` and
`test_read_format_rar_encryption_data.rar.uu`, decoded), which is distributed
under the BSD-2-Clause license. `rar_enc.rar` encrypts member data only; its
password is `12345678`, as documented in the matching libarchive test.

They are used by `src-tauri/src/archive/mod.rs` to cover RAR listing,
extraction, symlink skipping and the password flow; no RAR writer exists in
this project (the UnRAR license forbids building one).

`rar5_multi.part1.rar` … `part4.rar` are the four volumes of
`test_read_format_rar5_multiarchive_solid.part0N.rar.uu` from the same libarchive
test suite (BSD-2-Clause). A member's data starts in one volume and ends in
another, so they cover volume discovery: `rar_volumes_are_joined` extracts the set
both from part 1 and from part 3, and asserts part 1 alone fails.
