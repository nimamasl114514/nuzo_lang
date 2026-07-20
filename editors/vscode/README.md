# Nuzo Lang for Visual Studio Code

Provides language support for the [Nuzo Lang](https://github.com/nuzo/nuzo_lang) programming language.

## Features

- Syntax highlighting for `.nuzo` and `.nz` files
- Code snippets for common constructs (fn, if, match, struct, etc.)
- Language Server Protocol support:
  - Keyword completion
  - Hover documentation
  - Compiler error diagnostics
- Commands:
  - **Nuzo: Run File** — Execute the current `.nuzo` file
  - **Nuzo: Compile File** — Compile and show errors
  - **Nuzo: Show Disassembly** — Display bytecode disassembly

## Requirements

The `nuzo_run` executable must be available. Either:
- Build it: `cargo build --release` in the nuzo_lang project root
- Or set `nuzo.executablePath` to the full path of `nuzo_run`/`nuzo_run.exe`

## Extension Settings

This extension contributes the following settings:

- `nuzo.executablePath`: Path to `nuzo_run` executable. If empty, searches PATH.
- `nuzo.enableLanguageServer`: Enable/disable the language server (default: true).

## Usage

1. Open a `.nuzo` file
2. Syntax highlighting applies automatically
3. Use `Ctrl+Shift+P` and type "Nuzo" to see available commands
4. Right-click in editor for context menu commands
5. Press `F5` to run the current `.nuzo` file

## Snippets

| Prefix | Description |
|--------|-------------|
| `fn` | Function declaration |
| `if` | If statement |
| `ife` | If/else statement |
| `for` | For loop |
| `while` | While loop |
| `match` | Match expression |
| `struct` | Struct declaration |
| `enum` | Enum declaration |
| `let` | Let binding |
| `impl` | Impl block |

## Development

```bash
cd editors/vscode
npm install
npm run compile
```

Press `F5` to launch an Extension Development Host for testing.

## License

MIT
