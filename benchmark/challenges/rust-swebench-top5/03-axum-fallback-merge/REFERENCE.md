# Reference

- Dataset: `user2f86/rustbench`
- Split: `train`
- Instance: `tokio-rs__axum-529`
- Repository: `tokio-rs/axum`
- Base commit: `f9a437d0813c0e2f784ca90eccccb4fbad9126cb`
- Dataset version: `0.3`
- Issue: https://github.com/tokio-rs/axum/issues/488
- Pull request: https://github.com/tokio-rs/axum/pull/529

## Problem Statement

See [`upstream/problem_statement.md`](upstream/problem_statement.md).

## Gold Patch Files

- `axum/CHANGELOG.md`
- `axum/src/docs/routing/fallback.md`
- `axum/src/docs/routing/merge.md`
- `axum/src/docs/routing/nest.md`
- `axum/src/routing/mod.rs`

## Dataset Fail-to-Pass Tests

- `routing::tests::merging_routers_with_fallbacks_panics - should panic`
- `routing::tests::nesting_router_with_fallbacks_panics - should panic`

## Dataset Pass-to-Pass Tests

- `error_handling::traits`
- `body::stream_body::stream_body_traits`
- `extract::connect_info::traits`
- `extract::extractor_middleware::traits`
- `extract::path::de::tests::test_parse_struct`
- `extract::form::tests::test_form_query`
- `extract::path::de::tests::test_parse_map`
- `extract::form::tests::test_incorrect_content_type`
- `extract::form::tests::test_form_body`
- `extract::path::de::tests::test_parse_seq`
- `extract::path::de::tests::test_parse_single_value`
- `extract::request_parts::body_stream_traits`
- `extract::query::tests::test_query`
- `handler::into_service::traits`
- `response::headers::tests::invalid_header_name`
- `response::headers::tests::invalid_header_value`
- `response::headers::tests::vec_of_header_name_and_value`
- `response::headers::tests::vec_of_strings`
- `response::headers::tests::with_body`
- `response::headers::tests::with_status_and_body`
- `routing::into_make_service::tests::traits`
- `response::tests::test_merge_headers`
- `routing::route::tests::traits`
- `routing::method_routing::tests::layer`
- `routing::method_routing::tests::get_handler`
- `...` (173 more pass-to-pass checks in dataset metadata)
