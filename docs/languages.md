# Languages

[日本語](ja/languages.md)

This page lists the languages whose comments are translated on Hover, the rules and limits of comment extraction, and the language server settings of each language.

## Supported languages

Automatic comment Hover supports Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, Python, JSON, JSONC, YAML, TOML, shell scripts, PowerShell, Dockerfile, and Docker Compose, including Python docstrings. Markdown requires a prose selection before background translation; paragraph Hover alone does not start it.

## Comment extraction

### C#

C# supports `//`, `/* ... */`, `///`, and `/** ... */`. Adjacent line comments of the same kind and indentation are translated together. XML documentation is displayed as tagged text in Hover; the model is instructed to retain tags and attributes. There is no XML renderer or structural validation. Comments inside ambiguous syntax-error regions are skipped by automatic Hover; manual selection translation remains available.

### C and C++

C/C++ supports ordinary and Doxygen comments (`///`, `//!`, `/**`, `/*!`, including trailing `<` forms). Documentation commands such as `@param` remain in the displayed text; the model is instructed to retain commands and parameter names. Syntax-error regions are skipped, so recognized comments after macros or incomplete expressions remain available. If an error region contains a quote or block-comment delimiter, extraction stops there to avoid treating unfinished literal contents as comments. Use manual selection for those ambiguous regions.

### Python

Python supports `#` comments and docstrings at the beginning of a module, class, or function body. Ordinary, raw, Unicode, parenthesized, and implicitly concatenated string docstrings are supported; assigned strings, f-strings, and bytes are not automatic targets. Quotes and string prefixes are removed from docstrings; source escapes and indentation are passed through without evaluating Python. Shebangs are skipped. Syntax-error regions are skipped. If an error region contains a quote, extraction stops there to avoid translating apparent comments inside unfinished strings; manual selection remains available.

### JSON, YAML, and TOML

JSON and JSONC support `//` and `/* ... */`; YAML and TOML support `#`. Adjacent line comments of the same kind and indentation are translated together. Only comments are automatic targets. To translate a key or a string value, select it and use `Translate selection to …`.

Text inside a string is not treated as a comment: JSON strings, quoted YAML scalars and block scalars (`|` and `>`), and the four kinds of TOML strings. In YAML, a `#` that does not follow whitespace, as in `a#b` or `http://a/#frag`, is not a comment either.

These formats are read token by token. In JSON and TOML, errors in the structure, such as a missing comma, an unfinished value, or an unclosed bracket, therefore do not hide comments. An unclosed one-line string ends at the end of its line: its contents are not read as comments, and later lines are unaffected. An unclosed JSON block comment, or an unclosed multi-line TOML string, hides everything after it. In JSON, text in single quotes is skipped like a string. In TOML, a quote in the middle of a bare value, as in `don't`, also starts a string. In YAML, extraction stops at the first error the scanner reports, such as an unclosed quote, a tab used for indentation, or a line that is neither a key nor a value. Comments before the error remain available; one on the line of the error, or just before it, can be missing. The scanner also reports an error for a multi-line quoted value whose continuation lines are indented only one column deeper than its key. Manual selection translation remains available.

### Shell scripts

Shell scripts support `#` comments. Adjacent line comments of the same indentation are translated together. A shebang on the first line is skipped. sh, Bash, and zsh scripts are read in the same way. Where the shells read a script differently, the server follows Bash, and leaves a comment out rather than take text that Bash reads as data.

A `#` starts a comment at the start of a line, and after a blank, `;`, or `&`. A `#` inside a word, as in `a#b` or `${x#y}`, does not. Text inside quotes, here-documents, `${ ... }`, and arithmetic such as `$(( ... ))` and `(( ... ))` is not treated as a comment. A comment inside a command substitution, `$( ... )` or backquotes, is a comment, also when the substitution is inside a quoted string. A substitution inside a here-document is not read.

The following comments are not automatic targets:

- A comment right after `(`, `)`, `|`, `<`, or `>`, with no blank between. Bash reads a comment there, and zsh can read a part of a pattern, as in `(#b)` and `(a|b)#`.
- Inside backquotes, a comment that the closing backquote ends, and every comment after a backslash followed by `\`, a backquote, `$`, or `"`.

The structure of a script is not checked, so an `if` without its `fi`, or a `)` without its `(`, hides nothing. An unclosed quote, `${`, or `$((` hides everything after it, and so does a here-document without its ending line.

Extraction stops at the following, where the server cannot tell how a shell reads on. The comments before that point remain available.

- `${ command; }`, which Bash 5.3 reads as commands and older shells do not.
- A here-document whose delimiter holds `$` or a backquote outside single quotes, or has `(` right after it.
- A here-document that is still open at the `)` of the `$( ... )` around it.
- A backslash at the end of a line that splits an operator, such as `&&`, `<<`, or `$(`, over two lines.
- `((` that opens two subshells instead of arithmetic, when the text up to the first `)` that balances holds `#`, `[`, `<<`, or `case`.
- `<<` inside the parentheses of a pattern, as in `@(a|<<b)`, where zsh can begin a here-document.
- Inside `$( ... )`, a `case` pattern in parentheses with a word right after it, as in `(a)echo`, and a `)` that closes the substitution before the `esac` of a `case` in it.

Manual selection translation remains available.

### PowerShell

PowerShell supports `#` comments and `<# ... #>` block comments. Adjacent line comments of the same indentation are translated together, and each block comment is handled separately. A shebang on the first line and `#Requires` lines are skipped. Text inside single-quoted strings, double-quoted strings, and here-strings is not treated as a comment. A comment inside `$( ... )` within a double-quoted string is not an automatic target.

An unclosed block comment waits until closed. An unclosed string or here-string hides the comments after it. Manual selection translation remains available.

### Dockerfile and Docker Compose

In a Dockerfile, a comment is a line whose first character after its indentation is `#`. A `#` later in a line, as in `FROM a # text`, is not a comment. Comment lines between the continued lines of an instruction are comments. Adjacent comment lines of the same indentation are translated together.

Parser directives are skipped: the lines `# syntax=...`, `# escape=...`, and `# check=...` at the top of the file, before any other comment, blank line, or instruction. A shebang on the first line is skipped too.

The lines of a here-document are not comments. A here-document follows a `RUN`, `COPY`, or `ADD` instruction that has a word starting with `<<` or `<<-`, and runs to the line that holds only its delimiter. With `<<-`, tabs before the delimiter on that line are ignored. A here-document without that line runs to the end of the file. `<<<` does not start a here-document, and neither does `<<` inside quotes.

Docker Compose files are read as YAML, by the rules under [JSON, YAML, and TOML](#json-yaml-and-toml).

### Unclosed comments and JavaScript/TypeScript syntax errors

Unclosed block comments wait until closed. If a fatal JS/TS syntax error stops parsing, later comments are unavailable for automatic extraction. Fix the syntax or use manual selection translation.

## Language server settings

For C# files, install Zed's [C# language extension](https://github.com/zed-extensions/csharp). Its settings key is `CSharp`. If you configure `languages.CSharp.language_servers` explicitly, add `"translation-lsp"` alongside your existing C# server. This translation extension does not replace Roslyn or OmniSharp.

C and C++ are built into Zed. If you list their language servers explicitly, add `"translation-lsp"` under `languages.C.language_servers` and `languages."C++".language_servers`, alongside clangd.

Python is built into Zed. If you configure `languages.Python.language_servers`, add `"translation-lsp"` to that list alongside your existing Python servers.

JSON, JSONC, and YAML are built into Zed. For TOML files, install Zed's [TOML extension](https://github.com/zed-extensions/toml). The settings keys are `JSON`, `JSONC`, `YAML`, and `TOML`. If you list the language servers of one of these languages explicitly, add `"translation-lsp"` alongside your existing servers.

Shell Script is built into Zed, and covers files such as `.sh`, `.bash`, `.zsh`, `.bashrc`, and `.zshrc`. For PowerShell files, install Zed's [PowerShell extension](https://github.com/zed-extensions/powershell). For Dockerfile and Docker Compose files, install Zed's [Dockerfile extension](https://github.com/zed-extensions/dockerfile), which provides both languages. The settings keys are `Shell Script`, `PowerShell`, `Dockerfile`, and `Docker Compose`. If you list the language servers of one of these languages explicitly, add `"translation-lsp"` alongside your existing servers.

Zed uses the `JavaScript` settings for `.jsx` and `TSX` for `.tsx`.

The rule shared by every language is described under [Language server lists](configuration.md#language-server-lists).
