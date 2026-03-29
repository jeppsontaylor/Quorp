# Quorp Extensions

This directory contains extensions for Quorp that are largely maintained by the Quorp team. They currently live in the Quorp repository for ease of maintenance.

If you are looking for the Quorp extension registry, see the [`quorp-industries/extensions`](https://github.com/quorp-industries/extensions) repo.

## Structure

Currently, Quorp includes support for a number of languages without requiring installing an extension. Those languages can be found under [`crates/languages/src`](https://github.com/quorp-industries/quorp/tree/main/crates/languages/src).

Support for all other languages is done via extensions. This directory ([extensions/](https://github.com/quorp-industries/quorp/tree/main/extensions/)) contains some of the officially maintained extensions. These extensions use the same [quorp_extension_api](https://docs.rs/quorp_extension_api/latest/quorp_extension_api/) available to all [Quorp Extensions](https://quorp.dev/extensions) for providing [language servers](https://quorp.dev/docs/extensions/languages#language-servers), [tree-sitter grammars](https://quorp.dev/docs/extensions/languages#grammar) and [tree-sitter queries](https://quorp.dev/docs/extensions/languages#tree-sitter-queries).

You can find the other officially maintained extensions in the [quorp-extensions organization](https://github.com/quorp-extensions).

## Dev Extensions

See the docs for [Developing an Extension Locally](https://quorp.dev/docs/extensions/developing-extensions#developing-an-extension-locally) for how to work with one of these extensions.
