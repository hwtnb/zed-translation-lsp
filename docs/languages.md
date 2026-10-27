# Languages

[日本語](ja/languages.md)

This page lists the languages whose comments are translated on Hover, the rules and limits of comment extraction, and the language server settings of each language.

## Supported languages

Automatic comment Hover supports Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, Python, Go, Java, Kotlin, Swift, Ruby, PHP, HTML, CSS, SQL, JSON, JSONC, YAML, TOML, shell scripts, PowerShell, Dockerfile, and Docker Compose, including Python docstrings. Markdown requires a prose selection before background translation; paragraph Hover alone does not start it.

## Comment extraction

### General rules

The server finds comments by reading a document as its language reads it, without running it. Rust is read with the lexer of the Rust compiler, JavaScript and TypeScript with the Oxc parser, C, C++, C#, and Python with their Tree-sitter grammars, and YAML with a YAML scanner. The other languages are read by readers of the server's own that follow the tokenizer of the language. Text inside strings and other literals is not treated as a comment.

Line comments on consecutive lines are translated together when they are of the same kind, have the same indentation, and stand alone on their lines. A block comment, a docstring, and an HTML comment are each translated on their own.

Where the text is unfinished, or where the server cannot read it as the language does, the result is one of the following. The section of each language says which apply there.

- An unclosed block comment is not translated, and no comment after it is found, until it is closed.
- An unclosed string or other literal hides the comments after it, or ends at the end of its line, as the language reads it.
- Around a syntax error, or where the language can read the text in more than one way, a comment is left out, or extraction stops there. The comments before that point remain available.
- In the forms described under Java, Ruby, HTML, and SQL, text that the language does not read as a comment can be taken as one.

In all of these cases, select the text and use `Translate selection to …` to translate it.

#### Documents whose comments cannot be read

The server reads the comments of a document when a Hover or a translation first needs them, and again after an edit. For C, C++, C#, and Python, the reading stops once it has taken longer than `comment_parse_timeout_ms`, which is 1 second by default. The other languages have no such limit.

When the reading stops at the limit, or fails inside the server, the server keeps running and shows one warning that names the file. From then on no comment is recognized in that document, and editing it does not start another reading. Manual selection translation keeps working there; a selection that is exactly a comment is sent as selected, with its comment marks. Close the document and open it again to have its comments read once more. Other documents are not affected.

### Rust

Rust supports `//` and `/* ... */`, which nest, and the documentation comments `///`, `//!`, `/** ... */`, and `/*! ... */`. A documentation comment is not translated together with an ordinary one. A shebang on the first line is skipped.

### JavaScript and TypeScript

JavaScript, JSX, TypeScript, and TSX support `//` and `/* ... */`, including JSDoc comments `/** ... */` and comments in JSX such as `{/* ... */}`. The file name gives the rules of modules and declarations, as for `.mjs`, `.cjs`, `.mts`, `.cts`, and `.d.ts`. Text inside strings, template literals, and regular expressions is not treated as a comment. A hashbang on the first line is skipped.

A syntax error that stops the parser hides the comments after it. Code nested more than about 100,000 levels deep, such as that many `[` in a row, stops the language server, because the parser reads each level of nesting by recursion. This also holds for the scripts of an HTML document.

### C#

C# supports `//`, `/* ... */`, `///`, and `/** ... */`. XML documentation is displayed as tagged text in Hover; the model is instructed to retain tags and attributes. There is no XML renderer or structural validation. Comments inside ambiguous syntax-error regions are skipped.

### C and C++

C/C++ supports ordinary and Doxygen comments (`///`, `//!`, `/**`, `/*!`, including trailing `<` forms). Documentation commands such as `@param` remain in the displayed text; the model is instructed to retain commands and parameter names. Syntax-error regions are skipped, so recognized comments after macros or incomplete expressions remain available. If an error region contains a quote or block-comment delimiter, extraction stops there to avoid treating unfinished literal contents as comments.

### Python

Python supports `#` comments and docstrings at the beginning of a module, class, or function body. Ordinary, raw, Unicode, parenthesized, and implicitly concatenated string docstrings are supported; assigned strings, f-strings, and bytes are not automatic targets. Quotes and string prefixes are removed from docstrings; source escapes and indentation are passed through without evaluating Python. Shebangs are skipped. Syntax-error regions are skipped. If an error region contains a quote, extraction stops there to avoid translating apparent comments inside unfinished strings.

### Go

Go supports `//` and `/* ... */`. Text inside strings, runes, and raw strings is not treated as a comment. Directives are skipped by the rule of Go's own `go/ast`: a `//` comment whose text right after the slashes is `line `, `extern `, `export `, or lowercase letters and digits followed by a colon and another such character, as in `//go:build` and `//nolint:errcheck`. An unclosed string or rune ends at the end of its line. An unclosed raw string or block comment hides everything after it.

### Java

Java supports `//` and `/* ... */`, and the documentation comments `/** ... */` and `///`. Consecutive `///` lines are translated together, apart from `//` lines. Text inside strings, text blocks, and character literals is not treated as a comment. An unclosed string or character literal ends at the end of its line. An unclosed text block or block comment hides everything after it.

Java reads Unicode escapes such as `\u002F` before it reads tokens; the server does not. A comment written with them is not found, and a quote written as one neither opens nor closes a string.

### Kotlin

Kotlin supports `//` and `/* ... */`, which nest, including KDoc comments `/** ... */`. Text inside strings, raw strings `"""..."""`, character literals, and names between backticks is not treated as a comment. A comment inside `${ ... }` of a string is a comment; in a string that starts with several `$`, such as `$$"..."`, a template starts with as many `$`. A shebang on the first line is skipped. An unclosed string or character literal ends at the end of its line. An unclosed raw string or block comment hides everything after it.

### Swift

Swift supports `//` and `/* ... */`, which nest, including the documentation comments `///` and `/** ... */`. Text inside strings, multi-line strings, raw strings such as `#"..."#`, regular expressions between `#/` and `/#`, and names between backticks is not treated as a comment. A comment inside `\( ... )` of a string is a comment. A shebang on the first line is skipped. An unclosed string ends at the end of its line. An unclosed multi-line string or block comment hides everything after it. An unclosed `#/` hides the rest of its line, or everything after it when only blanks and a line break follow `#/`.

A regular expression between single slashes, as in `/a+/`, is not followed, because Swift decides whether `/` starts one by the code before it. A quote inside one can hide the comments after it on its line.

### Ruby

Ruby supports `#` comments and `=begin` ... `=end` blocks. A shebang on the first line is skipped, and so is everything after `__END__`.

Text inside strings, symbols, regular expressions, `%` literals such as `%w[...]`, character literals such as `?#`, and here-documents is not treated as a comment. A comment inside `#{ ... }` of a string is a comment.

Whether `/`, `%`, `?`, and `<<` start a literal depends on what precedes them, and Ruby decides it partly by whether a word is a local variable. The server follows the same rule. A word is a local variable within its scope after it has been assigned, also as one of the targets in `a, b = list`; named as a parameter, but not in a default value; named as the variable of `for` or of `rescue => error`; bound in a pattern after `in`; or named by a group of a regular expression before `=~`, as in `/(?<year>\d+)/ =~ text`. A `def`, `class`, or `module` starts a scope that sees no variable outside it, and a block or lambda a scope that sees those outside it. In `x /2 # note`, for example, `# note` is a comment when `x` is a local variable, and part of a regular expression when `x` is a method.

The server follows the forms of these declarations that code commonly uses. A variable bound by `=>` after a value, as in `value => name`, is not followed, and in rarer forms the server can take a variable for a method or the reverse. A comment after `/`, `%`, `?`, or `<<` can then be left out, or part of a literal taken as a comment. An unclosed string, literal, here-document, or `=begin` hides everything after it.

### PHP

PHP supports `#`, `//`, `/* ... */`, and `/** ... */` in the code between `<?php`, `<?=`, or `<?` and `?>`. `<?` opens code as it does with PHP's default `short_open_tag`. The text outside the code, such as HTML, is not read, so an HTML comment there is not an automatic target.

`#[` starts an attribute, not a comment. `?>` ends a line comment as well as the code. Text inside strings, here-documents, and nowdocs is not treated as a comment. A comment inside `{$ ... }` of a string is a comment. Everything after `__halt_compiler` is skipped, unless the word names a property or a variable, as in `$object->__halt_compiler`. An unclosed string, here-document, or block comment hides everything after it.

### HTML and CSS

HTML supports `<!-- ... -->`. Inside `<script>`, the comments of JavaScript are read as in a JavaScript file, and inside `<style>`, those of CSS. A script is read when its `type` is empty, a JavaScript type, or `module`. Other scripts, such as JSON, are not. A syntax error that stops the reading of a script hides the comments after it in that script, as in a JavaScript file.

Text inside tags and their attribute values, and inside `<textarea>`, `<title>`, and the other elements whose text holds no markup, is not treated as a comment. A script ends where a browser ends it, also where `<!--` and `<script>` inside it keep a `</script>` in its text.

The server reads HTML without building the tree of the document. Where the tree changes how a browser reads the text, the server reads it as in the body of a document, with these exceptions. `<![CDATA[` runs to `]]>`, as in SVG and MathML, also elsewhere. The text of `<script>` and `<style>` inside SVG and MathML is read as JavaScript and CSS. An element such as `<textarea>` inside `<select>`, which a browser ignores there, still holds no comment. An unclosed comment or tag hides everything after it, and so does `<plaintext>`.

CSS supports `/* ... */`. Text inside strings and inside an unquoted `url( ... )` is not treated as a comment. An unclosed comment hides everything after it.

### SQL

SQL is read by the token rules of Zed's SQL extension, whatever the dialect. It supports `--` and `/* ... */`, which do not nest. Text inside strings in single quotes, and inside names in double quotes or backticks, is not treated as a comment. A quote doubled inside them stands for itself, and in `E'...'` a backslash escapes the next character. A quote or `/*` that is never closed opens no string or comment, so the comments after it remain available.

Text between `$$` or `$tag$`, such as the body of a PostgreSQL function, is read as SQL, so the comments in a function body are found. A string written between `$$` that holds a quote or `--` can therefore hide the comments after it or have part of its text taken as a comment.

The forms of other dialects are not followed. A MySQL `#` comment is not found. MySQL escapes a quote with a backslash before it; the server closes the string there. A `/*` inside a block comment does not nest as it does in PostgreSQL.

### JSON, YAML, and TOML

JSON and JSONC support `//` and `/* ... */`; YAML and TOML support `#`. Only comments are automatic targets. To translate a key or a string value, select it and use `Translate selection to …`.

Text inside a string is not treated as a comment: JSON strings, quoted YAML scalars and block scalars (`|` and `>`), and the four kinds of TOML strings. In YAML, a `#` that does not follow whitespace, as in `a#b` or `http://a/#frag`, is not a comment either.

These formats are read token by token. In JSON and TOML, errors in the structure, such as a missing comma, an unfinished value, or an unclosed bracket, therefore do not hide comments. An unclosed one-line string ends at the end of its line: its contents are not read as comments, and later lines are unaffected. An unclosed JSON block comment, or an unclosed multi-line TOML string, hides everything after it. In JSON, text in single quotes is skipped like a string. In TOML, a quote in the middle of a bare value, as in `don't`, also starts a string. In YAML, extraction stops at the first error the scanner reports, such as an unclosed quote, a tab used for indentation, or a line that is neither a key nor a value. Comments before the error remain available; one on the line of the error, or just before it, can be missing.

### Shell scripts

Shell scripts support `#` comments. A shebang on the first line is skipped. sh, Bash, and zsh scripts are read in the same way. Where the shells read a script differently, the server follows Bash, and leaves a comment out rather than take text that Bash reads as data.

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

### PowerShell

PowerShell supports `#` comments and `<# ... #>` block comments. A shebang on the first line and `#Requires` lines are skipped.

A `#` or `<#` starts a comment at the start of a line, after a blank, and after `(`, `)`, `{`, `}`, `,`, `;`, `|`, or `&`. It also does right after a string or a block comment that began at one of these places. A `#` after a backtick does not start a comment. Text inside single-quoted strings, double-quoted strings, here-strings, and `${ ... }` is not treated as a comment. The other quotation marks that PowerShell accepts, such as `‘`, `’`, `“`, and `”`, work as `'` and `"` do. A comment inside `$( ... )` within a double-quoted string is not an automatic target.

Where a `#` follows other text with no blank between, as in `a#b`, `$x#text`, or `${x}#text`, PowerShell reads a comment or a part of an argument, depending on the statement around it. The server does not decide between the two, and nothing on that line from the `#` on is an automatic target. The same holds for the rest of a line after `--%`, which a command takes as it is written, also where the `--%` ends a string.

The server follows PowerShell where it reads on although the text looks finished:

- In a double-quoted string and in a here-string in double quotes, `$( ... )` runs to its last parenthesis and `${ ... }` to its `}`, over quotes and over a line that would end the here-string.
- A backtick before a line break inside an argument, or inside a here-string in double quotes, escapes the line break, so the next line does not start a token or end the here-string. After CRLF it does, because the backtick takes only the CR.
- A here-string with other text than blanks after its opening `@"` or `@'` ends at `"@` or `'@` on that line, or with that line.
- `$$` and `@$` are one variable each, so `$$(` and `@${` open no `$( ... )` or `${ ... }`.

An unclosed string, here-string, or `${` hides the comments after it.

Extraction stops at the following, where the server cannot tell how PowerShell reads on. The comments before that point remain available.

- `<#` or a here-string right after other text, as in `$x=@"`.
- `$( ... )` right after other text, as in `a=$( ... )`, when it holds `#`, a backtick, `${`, `--%`, a here-string, or a string with a parenthesis in it.
- A backtick before a quote, `$`, `@`, `<`, `-`, or `u{`, when it follows a letter, a digit, `_`, `.`, `\`, `+`, `#`, or another backtick, as in ``a`"b``, or when its word holds `-name`, as in ``-a=`"b``. A type name such as ``List`1`` and a parameter of a command hold a backtick that escapes nothing.
- `${ ... }` in a word that holds `-name`, or after `@` inside a word, when it holds a quote, `#`, a backtick, or `%`. PowerShell can read its `{` as the start of a block there.
- A quote, `#`, `$`, `@`, `<`, a backtick, or one of `(`, `)`, `{`, `}`, `;`, `|`, and `&` after `[name,` on the same line. PowerShell can read the name of an assembly there, in which these open nothing. A backtick inside `[name` stops extraction too, before anything but a letter or a digit.
- `--%` with `|` or `&&` after it on its line, and `--%` after such a `#` or another `--%` on its line.
- A string, here-string, block comment, or `${ ... }` that starts after such a `#` or `--%` on its line and ends on a later line. The same holds for `$( ... )` right after other text, and for a backtick right after other text at the end of that line.

### Dockerfile and Docker Compose

In a Dockerfile, a comment is a line whose first character after its indentation is `#`. A `#` later in a line, as in `FROM a # text`, is not a comment. Comment lines between the continued lines of an instruction are comments.

Parser directives are skipped: the lines `# syntax=...`, `# escape=...`, and `# check=...` at the top of the file, before any other comment, blank line, or instruction. A shebang on the first line is skipped too.

The lines of a here-document are not comments. A here-document follows a `RUN`, `COPY`, or `ADD` instruction that has a word starting with `<<` or `<<-`, and runs to the line that holds only its delimiter. With `<<-`, tabs before the delimiter on that line are ignored. A here-document without that line runs to the end of the file. `<<<` does not start a here-document, and neither does `<<` inside quotes.

The delimiter is read as BuildKit reads it. Quotes around it or around a part of it are removed. A backslash is removed outside quotes and kept in single quotes. In double quotes it is removed only before `"`, `$`, or another backslash, so the delimiter of `<<"E\OF"` is `E\OF`. The escape directive does not change this.

BuildKit has no here-document in the following places, and reads the lines after them as instructions and comments. The server reads a here-document there as well, and takes a line as a comment only where both readings do.

- `<<-` followed by a blank and a word, as in `<<- EOF`. The word is the delimiter.
- An instruction with an unclosed quote, or with a `${` that BuildKit rejects, after a word that starts with `<<`. That word gives the delimiter.
- An empty delimiter, as in `<<""`. The here-document ends at an empty line.
- A delimiter that holds `<`, as in `<<"a<b"`.

Extraction stops where the delimiter cannot be read: where the unclosed quote or the rejected `${` is in the word of `<<` or in its delimiter, and where that word holds a `${name ... }` with more than a name, as in `<<EOF${x:-y}`. The comments before that point remain available.

Docker Compose files are read as YAML, by the rules under [JSON, YAML, and TOML](#json-yaml-and-toml).

## Language server settings

For C# files, install Zed's [C# language extension](https://github.com/zed-extensions/csharp). Its settings key is `CSharp`. If you configure `languages.CSharp.language_servers` explicitly, add `"translation-lsp"` alongside your existing C# server. This translation extension does not replace Roslyn or OmniSharp.

C and C++ are built into Zed. If you list their language servers explicitly, add `"translation-lsp"` under `languages.C.language_servers` and `languages."C++".language_servers`, alongside clangd.

Python is built into Zed. If you configure `languages.Python.language_servers`, add `"translation-lsp"` to that list alongside your existing Python servers.

Go and CSS are built into Zed, and Zed installs its HTML extension by default. For Ruby and PHP files, install Zed's [Ruby extension](https://github.com/zed-extensions/ruby) and [PHP extension](https://github.com/zed-extensions/php). The settings keys are `Go`, `Ruby`, `PHP`, `HTML`, and `CSS`. If you list the language servers of one of these languages explicitly, add `"translation-lsp"` alongside your existing servers.

For Java, Kotlin, Swift, and SQL files, install Zed's [Java extension](https://github.com/zed-extensions/java), [Kotlin extension](https://github.com/zed-extensions/kotlin), [Swift extension](https://github.com/zed-extensions/swift), and [SQL extension](https://github.com/zed-extensions/sql). The settings keys are `Java`, `Kotlin`, `Swift`, and `SQL`. If you list the language servers of one of these languages explicitly, add `"translation-lsp"` alongside your existing servers.

JSON, JSONC, and YAML are built into Zed. For TOML files, install Zed's [TOML extension](https://github.com/zed-extensions/toml). The settings keys are `JSON`, `JSONC`, `YAML`, and `TOML`. If you list the language servers of one of these languages explicitly, add `"translation-lsp"` alongside your existing servers.

Shell Script is built into Zed, and covers files such as `.sh`, `.bash`, `.zsh`, `.bashrc`, and `.zshrc`. For PowerShell files, install Zed's [PowerShell extension](https://github.com/zed-extensions/powershell). For Dockerfile and Docker Compose files, install Zed's [Dockerfile extension](https://github.com/zed-extensions/dockerfile), which provides both languages. The settings keys are `Shell Script`, `PowerShell`, `Dockerfile`, and `Docker Compose`. If you list the language servers of one of these languages explicitly, add `"translation-lsp"` alongside your existing servers.

Zed uses the `JavaScript` settings for `.jsx` and `TSX` for `.tsx`.

The rule shared by every language is described under [Language server lists](configuration.md#language-server-lists).
