(function () {
  "use strict";

  var THEME_KEY = "anqa.theme";
  var ACCENT_KEY = "anqa.accent";
  var THEMES = ["light", "dark", "oled"];
  var ACCENTS = ["blue", "orange"];
  var root = document.documentElement;

  function valid(value, allowed) {
    return allowed.indexOf(value) !== -1;
  }

  function stored(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (error) {
      return null;
    }
  }

  function store(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch (error) {
      /* Storage can be unavailable in private modes. */
    }
  }

  function initialTheme() {
    var saved = stored(THEME_KEY);
    if (valid(saved, THEMES)) {
      return saved;
    }
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }

  function initialAccent() {
    var saved = stored(ACCENT_KEY);
    return valid(saved, ACCENTS) ? saved : "blue";
  }

  var current = {
    theme: initialTheme(),
    accent: initialAccent(),
  };

  function apply(next) {
    root.setAttribute("data-theme", next.theme);
    root.setAttribute("data-accent", next.accent);
  }

  function set(theme, accent) {
    if (valid(theme, THEMES)) {
      current.theme = theme;
    }
    if (valid(accent, ACCENTS)) {
      current.accent = accent;
    }
    apply(current);
    store(THEME_KEY, current.theme);
    store(ACCENT_KEY, current.accent);
    window.dispatchEvent(
      new CustomEvent("anqa:themechange", {
        detail: { theme: current.theme, accent: current.accent },
      }),
    );
  }

  apply(current);

  window.AnqaTheme = {
    get: function () {
      return { theme: current.theme, accent: current.accent };
    },
    set: set,
  };
})();
