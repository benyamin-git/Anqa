(function () {
  "use strict";

  var loginForm = document.getElementById("login-form");
  var settingsDialog = document.getElementById("settings-dialog");
  var settingsForm = document.getElementById("settings-form");
  var settingsButton = document.getElementById("settings-button");
  var settingsClose = document.getElementById("settings-close");

  if (loginForm) {
    loginForm.addEventListener("submit", function (event) {
      event.preventDefault();
    });
  }

  if (!settingsDialog || !settingsForm || !settingsButton || !window.AnqaTheme) {
    return;
  }

  function syncSettings() {
    var current = window.AnqaTheme.get();
    var theme = settingsForm.querySelector(
      'input[name="theme"][value="' + current.theme + '"]',
    );
    var accent = settingsForm.querySelector(
      'input[name="accent"][value="' + current.accent + '"]',
    );
    if (theme) {
      theme.checked = true;
    }
    if (accent) {
      accent.checked = true;
    }
  }

  function updateTheme() {
    var theme = settingsForm.querySelector('input[name="theme"]:checked');
    var accent = settingsForm.querySelector('input[name="accent"]:checked');
    if (theme && accent) {
      window.AnqaTheme.set(theme.value, accent.value);
    }
  }

  settingsButton.addEventListener("click", function () {
    syncSettings();
    settingsDialog.showModal();
    var focused = settingsForm.querySelector(
      'input[name="theme"]:checked, input[name="accent"]:checked',
    );
    if (focused) {
      focused.focus();
    }
  });

  if (settingsClose) {
    settingsClose.addEventListener("click", function () {
      settingsDialog.close();
    });
  }

  settingsForm.addEventListener("change", function (event) {
    if (event.target.name === "theme" || event.target.name === "accent") {
      updateTheme();
    }
  });

  settingsDialog.addEventListener("click", function (event) {
    if (event.target === settingsDialog) {
      settingsDialog.close();
    }
  });
})();
