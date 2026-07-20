# GUI Tests

Run with the nuzo_gui binary:

```bash
cargo run -p nuzo_gui -- tests/gui/test_counter.nuzo
cargo run -p nuzo_gui -- tests/gui/test_basic.nuzo
```

The script must define a `render()` function that is called every frame.

## Available Functions

| English | 中文 | Description |
|---------|------|-------------|
| `heading(text)` | `标题(text)` | Section heading |
| `label(text)` | `标签(text)` | Text label |
| `button(text)` | `按钮(text)` | Clickable button, returns true when clicked |
| `separator()` | `分割线()` | Horizontal separator |
| `checkbox(text, checked)` | `复选框(text, checked)` | Checkbox, returns new state |
| `input(text)` | `输入框(text)` | Single-line text input, returns current text |
| `textarea(text)` | `多行输入框(text)` | Multi-line text input |
| `slider(value, min, max)` | `滑块(value, min, max)` | Slider widget |
| `progress(fraction)` | `进度条(fraction)` | Progress bar (0.0 - 1.0) |
| `tooltip(text)` | `工具提示(text)` | Tooltip on hover |
| `radio(text, selected)` | `单选框(text, selected)` | Radio button |
| `light_theme()` | `亮色主题()` | Switch to light theme |
| `dark_theme()` | `暗色主题()` | Switch to dark theme |
| `theme(name)` | `主题(name)` | Set theme by name ("light"/"dark") |
| `quit()` | `退出()` | Close the window |
