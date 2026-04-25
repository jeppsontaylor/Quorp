# Reference

- Dataset: `user2f86/rustbench`
- Split: `train`
- Instance: `bincode-org__bincode-475`
- Repository: `bincode-org/bincode`
- Base commit: `f33abb21b45ff20b63be2a5ab134fce0d6d86d59`
- Dataset version: `1.0`
- Issue: https://github.com/bincode-org/bincode/issues/474
- Pull request: https://github.com/bincode-org/bincode/pull/475

## Problem Statement

See [`upstream/problem_statement.md`](upstream/problem_statement.md).

## Gold Patch Files

- `Cargo.toml`
- `src/features/serde/de_owned.rs`

## Dataset Fail-to-Pass Tests

- `varint::decode_unsigned::test_decode_u32`
- `varint::decode_unsigned::test_decode_u16`
- `varint::decode_unsigned::test_decode_u128`
- `varint::decode_unsigned::test_decode_u64`
- `varint::encode_signed::test_encode_i128`
- `varint::encode_signed::test_encode_i16`
- `varint::encode_signed::test_encode_i32`
- `varint::encode_signed::test_encode_i64`
- `varint::encode_unsigned::test_encode_u128`
- `varint::encode_unsigned::test_encode_u16`
- `varint::encode_unsigned::test_encode_u32`
- `varint::encode_unsigned::test_encode_u64`
- `test_vec`
- `test_container_limits`
- `test_alloc_commons`
- `test_atomic_commons`
- `test_array`
- `test_duration_out_of_range`
- `test_duration_wrapping`
- `test_option_slice`
- `test_option_str`
- `test_refcell_already_borrowed`
- `test_slice`
- `test_str`
- `test_numbers`
- `test_c_style_enum`
- `test_decode`
- `test_decode_enum_struct_variant`
- `test_decode_enum_tuple_variant`
- `test_decode_enum_unit_variant`
- `test_decode_tuple`
- `test_empty_enum_decode`
- `test_encode`
- `test_encode_decode_str`
- `test_encode_enum_struct_variant`
- `test_encode_enum_tuple_variant`
- `test_encode_enum_unit_variant`
- `test_encode_tuple`
- `test_macro_newtype`
- `issue_431::test`
- `issue_459::test_issue_459`
- `issue_467::test_issue_467`
- `derive::test_serde_derive`
- `test_serde_round_trip`
- `test_serialize_deserialize_borrowed_data`
- `test_serialize_deserialize_owned_data`
- `test_std_cursor`
- `test_system_time_out_of_range`
- `test_std_file`
- `test_std_commons`
- `src/config.rs - config::Configuration<E,I,A,L>::with_variable_int_encoding (line 107)`
- `src/config.rs - config (line 7)`
- `src/de/mod.rs - de::Decoder::unclaim_bytes_read (line 89)`
- `src/config.rs - config::Configuration<E,I,A,L>::with_variable_int_encoding (line 125)`
- `src/features/serde/mod.rs - features::serde (line 15)`
- `src/enc/write.rs - enc::write::SliceWriter (line 17)`
- `src/lib.rs - spec (line 269)`
- `src/enc/encoder.rs - enc::encoder::EncoderImpl (line 12)`
- `src/lib.rs - spec (line 285)`
- `src/lib.rs - spec (line 167)`
- `src/lib.rs - spec (line 208)`
- `src/de/decoder.rs - de::decoder::DecoderImpl (line 15)`
- `src/lib.rs - spec (line 246)`
- `src/lib.rs - spec (line 298)`
- `src/lib.rs - (line 26)`
- `src/lib.rs - readme (line 185)`

## Dataset Pass-to-Pass Tests

- none listed
