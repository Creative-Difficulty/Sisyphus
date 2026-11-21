//! Sorting apps into coarse classes by their id. Unknown apps are
//! [`AppClass::Other`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppClass {
    Terminal,
    Editor,
    Browser,
    Other,
}

const TERMINALS: &[&str] = &[
    "alacritty",
    "blackbox",
    "cmd",
    "conemu",
    "conemu64",
    "conhost",
    "console",
    "contour",
    "cool-retro-term",
    "deepin-terminal",
    "foot",
    "footclient",
    "ghostty",
    "gnome-terminal",
    "gnome-terminal-server",
    "guake",
    "hyper",
    "iterm",
    "iterm2",
    "kgx",
    "kitty",
    "konsole",
    "lxterminal",
    "mate-terminal",
    "mintty",
    "openconsole",
    "powershell",
    "ptyxis",
    "pwsh",
    "qterminal",
    "rio",
    "rxvt",
    "st",
    "st-256color",
    "tabby",
    "terminal",
    "terminator",
    "terminology",
    "tilda",
    "tilix",
    "urxvt",
    "warp",
    "warp-terminal",
    "wezterm",
    "wezterm-gui",
    "windowsterminal",
    "xfce4-terminal",
    "xterm",
    "yakuake",
];

const EDITORS: &[&str] = &[
    "android-studio",
    "atom",
    "bbedit",
    "code",
    "code-insiders",
    "code-oss",
    "codium",
    "cursor",
    "emacs",
    "fleet",
    "geany",
    "gedit",
    "gnome-text-editor",
    "gvim",
    "kate",
    "lapce",
    "neovide",
    "notepad",
    "notepad++",
    "nova",
    "pulsar",
    "sublime-text",
    "sublime_text",
    "texteditor",
    "textmate",
    "vscode",
    "vscodium",
    "windsurf",
    "xcode",
    "zed",
    "zed-editor",
    // JetBrains IDEs, by their executable or app id.
    "clion",
    "datagrip",
    "goland",
    "idea",
    "idea64",
    "intellij-idea-community",
    "intellij-idea-ultimate",
    "phpstorm",
    "pycharm",
    "pycharm64",
    "rider",
    "rubymine",
    "rustrover",
    "webstorm",
];

const BROWSERS: &[&str] = &[
    "arc",
    "brave",
    "brave-browser",
    "chrome",
    "chromium",
    "chromium-browser",
    "edgemac",
    "epiphany",
    "falkon",
    "firefox",
    "firefox-esr",
    "floorp",
    "google-chrome",
    "google-chrome-stable",
    "kagimacos",
    "ladybird",
    "librewolf",
    "microsoft-edge",
    "microsoft-edge-stable",
    "msedge",
    "mullvad-browser",
    "opera",
    "qutebrowser",
    "safari",
    "thorium",
    "tor-browser",
    "vivaldi",
    "vivaldi-stable",
    "waterfox",
    "zen",
    "zen-browser",
];

/// Ids that need more than the last dot segment, such as macOS bundle ids.
const BY_FULL_ID: &[(&str, AppClass)] = &[
    ("com.apple.terminal", AppClass::Terminal),
    ("com.googlecode.iterm2", AppClass::Terminal),
    ("net.kovidgoyal.kitty", AppClass::Terminal),
    ("com.mitchellh.ghostty", AppClass::Terminal),
    ("io.alacritty", AppClass::Terminal),
    ("com.github.wez.wezterm", AppClass::Terminal),
    ("dev.warp.warp-stable", AppClass::Terminal),
    ("co.zeit.hyper", AppClass::Terminal),
    ("org.gnome.console", AppClass::Terminal),
    ("com.raggesilver.blackbox", AppClass::Terminal),
    ("com.microsoft.vscode", AppClass::Editor),
    ("com.microsoft.vscodeinsiders", AppClass::Editor),
    ("com.vscodium", AppClass::Editor),
    ("com.todesktop.230313mzl4w4u92", AppClass::Editor), // Cursor
    ("com.exafunction.windsurf", AppClass::Editor),
    ("dev.zed.zed", AppClass::Editor),
    ("com.sublimetext.4", AppClass::Editor),
    ("com.apple.dt.xcode", AppClass::Editor),
    ("com.panic.nova", AppClass::Editor),
    ("org.gnome.texteditor", AppClass::Editor),
    ("com.apple.safari", AppClass::Browser),
    ("com.google.chrome", AppClass::Browser),
    ("org.mozilla.firefox", AppClass::Browser),
    ("com.brave.browser", AppClass::Browser),
    ("com.microsoft.edgemac", AppClass::Browser),
    ("company.thebrowser.browser", AppClass::Browser), // Arc
    ("com.kagi.kagimacos", AppClass::Browser),         // Orion
    ("com.operasoftware.opera", AppClass::Browser),
    ("com.vivaldi.vivaldi", AppClass::Browser),
];

/// Sort an app id (Wayland app id, X11 class, macOS bundle id or Windows
/// executable name) into a class.
pub fn classify(app_id: &str) -> AppClass {
    let id = normalize(app_id);
    if id.is_empty() {
        return AppClass::Other;
    }
    if let Some((_, class)) = BY_FULL_ID.iter().find(|(full, _)| *full == id) {
        return *class;
    }
    if id.starts_with("com.jetbrains.") || id.starts_with("jetbrains-") {
        return AppClass::Editor;
    }
    // `org.kde.konsole` → `konsole`, `org.mozilla.firefox` → `firefox`.
    let last = id.rsplit('.').next().unwrap_or(&id);
    for name in [id.as_str(), last] {
        if TERMINALS.contains(&name) {
            return AppClass::Terminal;
        }
        if EDITORS.contains(&name) {
            return AppClass::Editor;
        }
        if BROWSERS.contains(&name) {
            return AppClass::Browser;
        }
    }
    AppClass::Other
}

/// Lowercase, without a path, `.exe` or `.app`, and with spaces as dashes.
fn normalize(app_id: &str) -> String {
    let name = app_id.rsplit(['/', '\\']).next().unwrap_or(app_id);
    let mut id = name.trim().to_lowercase().replace(' ', "-");
    for suffix in [".exe", ".app", ".desktop"] {
        if let Some(stripped) = id.strip_suffix(suffix) {
            id = stripped.to_string();
        }
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_apps() {
        assert_eq!(classify("kitty"), AppClass::Terminal);
        assert_eq!(classify("org.kde.konsole"), AppClass::Terminal);
        assert_eq!(classify("com.mitchellh.ghostty"), AppClass::Terminal);
        assert_eq!(classify("WindowsTerminal.exe"), AppClass::Terminal);
        assert_eq!(classify("com.apple.Terminal"), AppClass::Terminal);
        assert_eq!(classify("Code"), AppClass::Editor);
        assert_eq!(classify("code-oss"), AppClass::Editor);
        assert_eq!(classify("dev.zed.Zed"), AppClass::Editor);
        assert_eq!(classify("jetbrains-rustrover"), AppClass::Editor);
        assert_eq!(classify("com.jetbrains.intellij"), AppClass::Editor);
        assert_eq!(
            classify("C:\\Program Files\\Cursor\\Cursor.exe"),
            AppClass::Editor
        );
        assert_eq!(classify("firefox"), AppClass::Browser);
        assert_eq!(classify("org.mozilla.firefox"), AppClass::Browser);
        assert_eq!(classify("chrome.exe"), AppClass::Browser);
        assert_eq!(classify("com.apple.Safari"), AppClass::Browser);
        assert_eq!(classify("Slack"), AppClass::Other);
        assert_eq!(classify(""), AppClass::Other);
    }
}
