#pragma once

#include <stddef.h>
#include <stdint.h>

// Portapapeles interno compartido entre Editor, Terminal y Explorador (Zero-Alloc)
constexpr size_t UI_CLIPBOARD_MAX = 512;

struct UiClipboard {
    char buf[UI_CLIPBOARD_MAX];
    size_t len;
};

inline UiClipboard& ui_clipboard_state() {
    static UiClipboard clip = {{0}, 0};
    return clip;
}

inline void clipboard_clear() {
    UiClipboard& clip = ui_clipboard_state();
    clip.len = 0;
    clip.buf[0] = '\0';
}

inline void clipboard_set(const char* text, size_t len) {
    UiClipboard& clip = ui_clipboard_state();
    if (!text || len == 0) {
        clipboard_clear();
        return;
    }
    size_t n = (len >= UI_CLIPBOARD_MAX) ? (UI_CLIPBOARD_MAX - 1) : len;
    for (size_t i = 0; i < n; ++i) {
        clip.buf[i] = text[i];
    }
    clip.buf[n] = '\0';
    clip.len = n;
}

inline void clipboard_set_cstr(const char* text) {
    if (!text) {
        clipboard_clear();
        return;
    }
    size_t len = 0;
    while (text[len] != '\0' && len < UI_CLIPBOARD_MAX - 1) {
        ++len;
    }
    clipboard_set(text, len);
}

inline const char* clipboard_get() {
    return ui_clipboard_state().buf;
}

inline size_t clipboard_len() {
    return ui_clipboard_state().len;
}
