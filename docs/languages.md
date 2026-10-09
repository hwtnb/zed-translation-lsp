# Languages

[日本語](ja/languages.md)

This page lists the languages whose comments are translated on Hover, the rules and limits of comment extraction, and the language server settings of each language.

## Supported languages

Automatic comment Hover supports Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, and Python, including Python docstrings. Markdown requires a prose selection before background translation; paragraph Hover alone does not start it.

## Comment extraction

### C#

C# supports `//`, `/* ... */`, `///`, and `/** ... */`. Adjacent line comments of the same kind and indentation are translated together. XML documentation is displayed as tagged text in Hover; the model is instructed to retain tags and attributes. There is no XML renderer or structural validation. Comments inside ambiguous syntax-error regions are skipped by automatic Hover; manual selection translation remains available.

### C and C++

C/C++ supports ordinary and Doxygen comments (`///`, `//!`, `/**`, `/*!`, including trailing `<` forms). Documentation commands such as `@param` remain in the displayed text; the model is instructed to retain commands and parameter names. Syntax-error regions are skipped, so recognized comments after macros or incomplete expressions remain available. If an error region contains a quote or block-comment delimiter, extraction stops there to avoid treating unfinished literal contents as comments. Use manual selection for those ambiguous regions.

### Python

Python supports `#` comments and docstrings at the beginning of a module, class, or function body. Ordinary, raw, Unicode, parenthesized, and implicitly concatenated string docstrings are supported; assigned strings, f-strings, and bytes are not automatic targets. Quotes and string prefixes are removed from docstrings; source escapes and indentation are passed through without evaluating Python. Shebangs are skipped. Syntax-error regions are skipped. If an error region contains a quote, extraction stops there to avoid translating apparent comments inside unfinished strings; manual selection remains available.

### Unclosed comments and JavaScript/TypeScript syntax errors

Unclosed block comments wait until closed. If a fatal JS/TS syntax error stops parsing, later comments are unavailable for automatic extraction. Fix the syntax or use manual selection translation.

## Language server settings

For C# files, install Zed's [C# language extension](https://github.com/zed-extensions/csharp). Its settings key is `CSharp`. If you configure `languages.CSharp.language_servers` explicitly, add `"translation-lsp"` alongside your existing C# server. This translation extension does not replace Roslyn or OmniSharp.

C and C++ are built into Zed. If you list their language servers explicitly, add `"translation-lsp"` under `languages.C.language_servers` and `languages."C++".language_servers`, alongside clangd.

Python is built into Zed. If you configure `languages.Python.language_servers`, add `"translation-lsp"` to that list alongside your existing Python servers.

Zed uses the `JavaScript` settings for `.jsx` and `TSX` for `.tsx`.

The rule shared by every language is described under [Language server lists](configuration.md#language-server-lists).
