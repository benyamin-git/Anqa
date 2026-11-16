(function () {
  "use strict";

  var POLL_MS = 2000;
  var STATUS_LABELS = {
    running: "Running",
    stopped: "Stopped",
    starting: "Starting",
    restarting: "Restarting",
    unhealthy: "Unhealthy",
    error: "Error",
    building: "Building",
    stopping: "Stopping",
  };
  var HEALTH_LABELS = {
    healthy: "Healthy",
    unhealthy: "Unhealthy",
    starting: "Starting",
  };
  var LIVE_STATUSES = new Set([
    "running",
    "starting",
    "restarting",
    "unhealthy",
    "stopping",
  ]);

  function byId(id) {
    return document.getElementById(id);
  }

  function make(tag, className, text) {
    var node = document.createElement(tag);
    if (className) {
      node.className = className;
    }
    if (text !== undefined) {
      node.textContent = text;
    }
    return node;
  }

  var els = {
    loginView: byId("login-view"),
    dashboardView: byId("dashboard-view"),
    loginForm: byId("login-form"),
    tokenInput: byId("token"),
    loginError: byId("login-error"),
    loginSubmit: byId("login-submit"),
    main: byId("main-content"),
    summary: byId("summary"),
    refreshButton: byId("refresh-button"),
    settingsButton: byId("settings-button"),
    logoutButton: byId("logout-button"),
    banner: byId("banner"),
    bannerMessage: byId("banner-message"),
    bannerRetry: byId("banner-retry"),
    loadingState: byId("loading-state"),
    emptyState: byId("empty-state"),
    botGrid: byId("bot-grid"),
    statusAnnouncer: byId("status-announcer"),
    settingsDialog: byId("settings-dialog"),
    settingsForm: byId("settings-form"),
    settingsClose: byId("settings-close"),
  };

  if (!els.loginForm || !els.dashboardView || !els.botGrid) {
    return;
  }

  var ui = {
    session: 0,
    auth: false,
    loginPending: false,
    logoutPending: false,
    botsRequest: null,
    pollTimer: null,
    cards: new Map(),
    pending: new Set(),
    autostartIntent: new Map(),
    lastBots: new Map(),
    hasRendered: false,
    cardSequence: 0,
    bannerSticky: false,
    bannerRetry: null,
  };

  async function api(path, options) {
    options = options || {};
    var headers = new Headers(options.headers || {});
    headers.set("Accept", "application/json");
    if (Object.prototype.hasOwnProperty.call(options, "body")) {
      headers.set("Content-Type", "application/json");
    }
    var response = await fetch(path, {
      method: options.method || "GET",
      headers: headers,
      body: options.body,
      credentials: "same-origin",
      cache: "no-store",
    });
    var data = null;
    var contentType = response.headers.get("content-type") || "";
    if (contentType.indexOf("application/json") !== -1) {
      try {
        data = await response.json();
      } catch (error) {
        data = null;
      }
    }
    return { response: response, data: data };
  }

  function errorMessage(data, response) {
    if (
      data &&
      data.error &&
      typeof data.error.message === "string" &&
      data.error.message
    ) {
      return data.error.message;
    }
    if (response && response.status === 401) {
      return "Your session expired. Sign in again.";
    }
    if (response && response.status === 409) {
      return "An operation is already running for this bot.";
    }
    if (response && response.status === 503) {
      return "Docker is unavailable.";
    }
    return response
      ? "Request failed (" + response.status + ")."
      : "The request failed.";
  }

  function hideLoginError() {
    els.loginError.hidden = true;
    els.loginError.textContent = "";
    els.tokenInput.removeAttribute("aria-invalid");
  }

  function showLoginError(message) {
    els.loginError.textContent = message;
    els.loginError.hidden = false;
    els.tokenInput.setAttribute("aria-invalid", "true");
  }

  function setLoginPending(pending) {
    ui.loginPending = pending;
    els.loginSubmit.disabled = pending;
    els.loginSubmit.textContent = pending ? "Signing in..." : "Sign in";
  }

  function showBanner(message, retry, sticky) {
    ui.bannerSticky = Boolean(sticky);
    ui.bannerRetry =
      typeof retry === "function"
        ? retry
        : function () {
            void refreshBots();
          };
    els.bannerMessage.textContent = message;
    els.banner.hidden = false;
  }

  function hideBanner(force) {
    if (ui.bannerSticky && !force) {
      return;
    }
    ui.bannerSticky = false;
    ui.bannerRetry = null;
    els.bannerMessage.textContent = "";
    els.banner.hidden = true;
  }

  function stopPolling() {
    if (ui.pollTimer !== null) {
      window.clearTimeout(ui.pollTimer);
      ui.pollTimer = null;
    }
  }

  function markLoadFailure() {
    els.loadingState.hidden = true;
    els.emptyState.hidden = true;
    if (!ui.hasRendered) {
      els.summary.textContent = "—";
    }
  }

  function scheduleNextPoll() {
    stopPolling();
    if (!ui.auth || document.hidden) {
      return;
    }
    ui.pollTimer = window.setTimeout(function () {
      ui.pollTimer = null;
      void refreshBots();
    }, POLL_MS);
  }

  function clearBotState() {
    ui.cards.forEach(function (card) {
      card.li.remove();
    });
    ui.cards.clear();
    ui.pending.clear();
    ui.autostartIntent.clear();
    ui.lastBots.clear();
    ui.hasRendered = false;
    els.botGrid.replaceChildren();
    els.statusAnnouncer.textContent = "";
  }

  function resetSessionUI() {
    ui.session += 1;
    ui.auth = false;
    ui.botsRequest = null;
    ui.logoutPending = false;
    ui.loginPending = false;
    stopPolling();
    clearBotState();
    hideBanner(true);
    els.dashboardView.hidden = true;
    els.loginView.hidden = false;
    els.loadingState.hidden = true;
    els.emptyState.hidden = true;
    els.summary.textContent = "Loading bots…";
    els.logoutButton.disabled = false;
    els.bannerRetry = null;
    if (els.settingsDialog && els.settingsDialog.open) {
      els.settingsDialog.close();
    }
  }

  function showLogin(message) {
    resetSessionUI();
    els.tokenInput.value = "";
    setLoginPending(false);
    if (message) {
      showLoginError(message);
    } else {
      hideLoginError();
    }
    window.setTimeout(function () {
      els.tokenInput.focus();
    }, 0);
  }

  function enterDashboard(focus) {
    ui.session += 1;
    ui.auth = true;
    els.loginView.hidden = true;
    els.dashboardView.hidden = false;
    hideLoginError();
    if (focus) {
      els.main.focus({ preventScroll: true });
    }
  }

  function prepareDashboard() {
    clearBotState();
    hideBanner(true);
    els.loadingState.hidden = false;
    els.emptyState.hidden = true;
    els.summary.textContent = "Loading bots…";
  }

  function hasPending(dir) {
    var prefix = encodeURIComponent(dir) + ":";
    var found = false;
    ui.pending.forEach(function (key) {
      if (key.indexOf(prefix) === 0) {
        found = true;
      }
    });
    return found;
  }

  function opKey(dir, action) {
    return encodeURIComponent(dir) + ":" + action;
  }

  function formatUptime(value) {
    var seconds = Number(value);
    if (!Number.isFinite(seconds) || seconds <= 0) {
      return "—";
    }
    if (seconds < 60) {
      return "<1m";
    }
    var minutes = Math.floor(seconds / 60);
    if (minutes < 60) {
      return minutes + "m";
    }
    var hours = Math.floor(minutes / 60);
    var minutesLeft = minutes % 60;
    if (hours < 24) {
      return minutesLeft ? hours + "h " + minutesLeft + "m" : hours + "h";
    }
    var days = Math.floor(hours / 24);
    var hoursLeft = hours % 24;
    return hoursLeft ? days + "d " + hoursLeft + "h" : days + "d";
  }

  function updateSummary(bots) {
    var live = 0;
    bots.forEach(function (bot) {
      if (LIVE_STATUSES.has(bot.status)) {
        live += 1;
      }
    });
    els.summary.textContent = live + "/" + bots.length + " live";
  }

  function statusLabel(status) {
    return STATUS_LABELS[status] || "Error";
  }

  function healthLabel(health) {
    return HEALTH_LABELS[health] || "";
  }

  function actionForStatus(status) {
    if (
      status === "running" ||
      status === "unhealthy" ||
      status === "restarting" ||
      status === "starting" ||
      status === "stopping"
    ) {
      return "stop";
    }
    return "start";
  }

  function createCard() {
    var sequence = ++ui.cardSequence;
    var card = {};
    var li = make("li", "bot-card");
    var title = make("h2", "bot-title");
    title.id = "bot-title-" + sequence;
    li.setAttribute("aria-labelledby", title.id);
    li.setAttribute("aria-busy", "false");

    var heading = make("div", "bot-heading");
    var dirText = make("p", "bot-dir");
    heading.append(title, dirText);

    var statusChip = make("span", "status-chip");
    var statusDot = make("span", "status-dot");
    statusDot.setAttribute("aria-hidden", "true");
    var statusText = make("span");
    statusChip.append(statusDot, statusText);

    var header = make("div", "bot-card-header");
    header.append(heading, statusChip);

    var meta = make("dl", "bot-meta");
    var uptimeDt = make("dt", null, "Uptime");
    var uptimeDd = make("dd");
    var healthDt = make("dt", null, "Health");
    var healthDd = make("dd");
    var healthBadge = make("span", "health-badge");
    var healthDot = make("span", "status-dot");
    healthDot.setAttribute("aria-hidden", "true");
    var healthText = make("span");
    healthBadge.append(healthDot, healthText);
    healthDd.append(healthBadge);
    meta.append(uptimeDt, uptimeDd, healthDt, healthDd);

    var error = make("p", "bot-error");
    error.hidden = true;

    var actions = make("div", "bot-actions");
    var action = make("button", "button button-primary");
    action.type = "button";
    action.addEventListener("click", function () {
      var verb = action.dataset.action;
      if (verb === "start" || verb === "stop") {
        void mutateBot(card.dir, verb);
      }
    });

    var switchLabel = make("label", "switch");
    var switchInput = document.createElement("input");
    switchInput.type = "checkbox";
    var switchTrack = make("span", "switch-track");
    var switchThumb = make("span", "switch-thumb");
    switchThumb.setAttribute("aria-hidden", "true");
    switchTrack.append(switchThumb);
    var switchText = make("span", null, "Autostart");
    var switchState = make("span", "switch-state");
    switchLabel.append(switchInput, switchTrack, switchText, switchState);
    switchInput.addEventListener("change", function () {
      if (ui.pending.has(opKey(card.dir, "autostart"))) {
        switchInput.checked = ui.autostartIntent.has(card.dir)
          ? ui.autostartIntent.get(card.dir)
          : Boolean(card.bot && card.bot.autostart);
        return;
      }
      void changeAutostart(card, switchInput.checked);
    });

    actions.append(action, switchLabel);
    li.append(header, meta, error, actions);

    card.li = li;
    card.title = title;
    card.dirText = dirText;
    card.statusChip = statusChip;
    card.statusText = statusText;
    card.meta = meta;
    card.uptimeDt = uptimeDt;
    card.uptimeDd = uptimeDd;
    card.healthDt = healthDt;
    card.healthDd = healthDd;
    card.healthBadge = healthBadge;
    card.healthText = healthText;
    card.error = error;
    card.actions = actions;
    card.action = action;
    card.switchInput = switchInput;
    card.switchState = switchState;
    card.bot = null;
    card.dir = "";

    return card;
  }

  function updateCard(card, bot) {
    card.bot = bot;
    card.dir = String(bot.dir);
    var name =
      typeof bot.name === "string" && bot.name.length > 0
        ? bot.name
        : card.dir;
    var status = STATUS_LABELS[bot.status] ? bot.status : "error";
    var label = statusLabel(status);
    var manifestError = bot.image === null || bot.image === undefined;
    var busy = Boolean(bot.busy) || hasPending(card.dir);

    card.title.textContent = name;
    card.dirText.textContent = "bots/" + card.dir;
    card.statusChip.className = "status-chip status-" + status;
    card.statusText.textContent = label;
    card.statusChip.setAttribute("aria-label", "Status: " + label);
    card.li.setAttribute("aria-busy", busy ? "true" : "false");

    card.meta.hidden = manifestError;
    if (!manifestError) {
      card.uptimeDd.textContent = formatUptime(bot.uptime_seconds);
      var health = HEALTH_LABELS[bot.health] ? bot.health : null;
      card.healthDt.hidden = !health;
      card.healthDd.hidden = !health;
      if (health) {
        card.healthBadge.className = "health-badge status-" + health;
        card.healthText.textContent = healthLabel(health);
      }
    }

    card.error.hidden = !bot.error;
    card.error.textContent = bot.error ? "Last error: " + String(bot.error) : "";

    var actionHadFocus = document.activeElement === card.action;
    var switchHadFocus = document.activeElement === card.switchInput;
    card.actions.hidden = manifestError;
    if (manifestError) {
      card.switchInput.disabled = true;
      if (actionHadFocus || switchHadFocus) {
        els.main.focus({ preventScroll: true });
      }
      return;
    }

    var verb = actionForStatus(bot.status);
    var controlDisabled =
      Boolean(bot.busy) ||
      bot.status === "building" ||
      bot.status === "stopping";
    card.action.hidden = false;
    card.action.dataset.action = verb;
    card.action.textContent = verb === "start" ? "Start" : "Stop";
    card.action.className =
      "button " + (verb === "start" ? "button-primary" : "button-tonal");
    card.action.setAttribute(
      "aria-label",
      (verb === "start" ? "Start " : "Stop ") + name,
    );
    card.action.disabled = controlDisabled;

    var autostart = ui.autostartIntent.has(card.dir)
      ? ui.autostartIntent.get(card.dir)
      : Boolean(bot.autostart);
    card.switchInput.checked = autostart;
    card.switchInput.disabled = controlDisabled;
    card.switchInput.setAttribute("aria-label", "Autostart for " + name);
    card.switchState.textContent = autostart ? "On" : "Off";

    if (controlDisabled && (actionHadFocus || switchHadFocus)) {
      els.main.focus({ preventScroll: true });
    }
  }

  function renderBots(bots) {
    var seen = new Set();
    var index = 0;

    bots.forEach(function (bot) {
      var dir = String(bot.dir);
      if (seen.has(dir)) {
        return;
      }
      seen.add(dir);
      var card = ui.cards.get(dir);
      if (!card) {
        card = createCard();
        ui.cards.set(dir, card);
      }
      var hadFocus = card.li.contains(document.activeElement);
      updateCard(card, bot);
      if (hadFocus && !card.li.contains(document.activeElement)) {
        els.main.focus({ preventScroll: true });
      }
      var current = els.botGrid.children[index] || null;
      if (current !== card.li) {
        els.botGrid.insertBefore(card.li, current);
      }
      index += 1;
    });

    Array.from(ui.cards.entries()).forEach(function (entry) {
      var dir = entry[0];
      var card = entry[1];
      if (seen.has(dir)) {
        return;
      }
      if (card.li.contains(document.activeElement)) {
        els.main.focus({ preventScroll: true });
      }
      card.li.remove();
      ui.cards.delete(dir);
    });

    while (els.botGrid.children.length > index) {
      els.botGrid.lastElementChild.remove();
    }
  }

  function reconcileAutostartIntents(bots) {
    var byDir = new Map();
    bots.forEach(function (bot) {
      byDir.set(String(bot.dir), bot);
    });
    Array.from(ui.autostartIntent.keys()).forEach(function (dir) {
      var bot = byDir.get(dir);
      if (!bot || !bot.busy) {
        ui.autostartIntent.delete(dir);
      }
    });
  }

  function updateAnnouncements(bots, announce) {
    var next = new Map();
    var messages = [];
    bots.forEach(function (bot) {
      var dir = String(bot.dir);
      next.set(dir, {
        name: bot.name,
        status: bot.status,
        error: bot.error,
      });
    });

    if (ui.hasRendered && announce) {
      next.forEach(function (current, dir) {
        var previous = ui.lastBots.get(dir);
        var name = current.name || dir;
        if (!previous) {
          messages.push(name + " added");
          return;
        }
        if (previous.status !== current.status) {
          messages.push(name + " is now " + statusLabel(current.status).toLowerCase());
        }
        if (current.error && current.error !== previous.error) {
          messages.push("Last error for " + name + ": " + current.error);
        }
      });
      ui.lastBots.forEach(function (previous, dir) {
        if (!next.has(dir)) {
          messages.push((previous.name || dir) + " removed");
        }
      });
    }

    ui.lastBots = next;
    ui.hasRendered = true;
    if (messages.length > 0) {
      els.statusAnnouncer.textContent = messages.join(". ");
    } else if (els.statusAnnouncer.textContent) {
      els.statusAnnouncer.textContent = "";
    }
  }

  function applyBots(bots, announce) {
    reconcileAutostartIntents(bots);
    hideBanner(false);
    renderBots(bots);
    els.loadingState.hidden = true;
    els.emptyState.hidden = bots.length !== 0;
    updateSummary(bots);
    updateAnnouncements(bots, announce);
  }

  function normalizeBots(value) {
    if (!Array.isArray(value)) {
      return [];
    }
    var seen = new Set();
    return value.filter(function (bot) {
      if (
        !bot ||
        typeof bot !== "object" ||
        typeof bot.dir !== "string" ||
        !bot.dir ||
        seen.has(bot.dir)
      ) {
        return false;
      }
      seen.add(bot.dir);
      return true;
    });
  }

  async function refreshBots(force) {
    if (!ui.auth) {
      return;
    }
    if (ui.botsRequest) {
      if (!force) {
        return ui.botsRequest;
      }
      try {
        await ui.botsRequest;
      } catch (error) {
        /* The forced refresh below retries the request. */
      }
      if (!ui.auth) {
        return;
      }
      return refreshBots(false);
    }

    var generation = ui.session;
    var request;
    request = (async function () {
      try {
        var result = await api("/api/bots");
        if (ui.session !== generation) {
          return;
        }
        if (result.response.status === 401) {
          showLogin("Your session expired. Sign in again.");
          return;
        }
        if (!result.response.ok) {
          markLoadFailure();
          showBanner(errorMessage(result.data, result.response), null, false);
          return;
        }
        var bots = normalizeBots(result.data && result.data.bots);
        applyBots(bots, true);
      } catch (error) {
        if (ui.session !== generation) {
          return;
        }
        markLoadFailure();
        showBanner("Cannot reach Anqa. Retrying...", null, false);
      } finally {
        if (ui.botsRequest === request) {
          ui.botsRequest = null;
        }
        if (ui.session === generation) {
          scheduleNextPoll();
        }
      }
    })();

    ui.botsRequest = request;
    return request;
  }

  async function bootstrap() {
    var generation = ui.session;
    try {
      var result = await api("/api/bots");
      if (ui.session !== generation || ui.loginPending) {
        return;
      }
      if (result.response.status === 401) {
        showLogin();
        return;
      }
      enterDashboard(false);
      if (!result.response.ok) {
        markLoadFailure();
        showBanner(errorMessage(result.data, result.response), null, false);
        scheduleNextPoll();
        return;
      }
      var bots = normalizeBots(result.data && result.data.bots);
      applyBots(bots, false);
      scheduleNextPoll();
    } catch (error) {
      if (ui.session !== generation || ui.loginPending) {
        return;
      }
      showLogin("Cannot reach Anqa. Check the server and try again.");
    }
  }

  async function handleLogin(event) {
    event.preventDefault();
    if (ui.loginPending) {
      return;
    }
    var token = els.tokenInput.value;
    if (!token.trim()) {
      showLoginError("Enter the dashboard token.");
      els.tokenInput.focus();
      return;
    }

    hideLoginError();
    setLoginPending(true);
    try {
      var result = await api("/api/login", {
        method: "POST",
        body: JSON.stringify({ token: token }),
      });
      if (!result.response.ok) {
        showLoginError(
          result.response.status === 401
            ? "That token was not accepted."
            : errorMessage(result.data, result.response),
        );
        els.tokenInput.focus();
        els.tokenInput.select();
        return;
      }
      els.tokenInput.value = "";
      prepareDashboard();
      enterDashboard(true);
      await refreshBots();
    } catch (error) {
      showLoginError("Cannot reach Anqa. Check the server and try again.");
      els.tokenInput.focus();
    } finally {
      setLoginPending(false);
    }
  }

  async function handleLogout() {
    if (ui.logoutPending || !ui.auth) {
      return;
    }
    if (els.settingsDialog && els.settingsDialog.open) {
      els.settingsDialog.close();
    }
    ui.logoutPending = true;
    els.logoutButton.disabled = true;
    try {
      var result = await api("/api/logout", {
        method: "POST",
        body: "{}",
      });
      if (result.response.ok || result.response.status === 401) {
        showLogin();
        return;
      }
      showBanner(
        "Could not sign out: " + errorMessage(result.data, result.response),
        function () {
          void handleLogout();
        },
        true,
      );
    } catch (error) {
      showBanner(
        "Could not sign out. Check the connection and try again.",
        function () {
          void handleLogout();
        },
        true,
      );
    } finally {
      ui.logoutPending = false;
      if (!els.dashboardView.hidden) {
        els.logoutButton.disabled = false;
      }
    }
  }

  function markCardBusy(card) {
    if (!card) {
      return;
    }
    var busy =
      (card.bot && Boolean(card.bot.busy)) || hasPending(card.dir);
    card.li.setAttribute("aria-busy", busy ? "true" : "false");
  }

  async function mutateBot(dir, verb) {
    if (!ui.auth || (verb !== "start" && verb !== "stop")) {
      return;
    }
    var key = opKey(dir, verb);
    if (ui.pending.has(key)) {
      return;
    }
    var card = ui.cards.get(dir);
    var name = card && card.bot ? card.bot.name : dir;
    ui.pending.add(key);
    markCardBusy(card);
    try {
      var result = await api(
        "/api/bots/" + encodeURIComponent(dir) + "/" + verb,
        {
          method: "POST",
          body: "{}",
        },
      );
      if (result.response.status === 401) {
        showLogin("Your session expired. Sign in again.");
        return;
      }
      if (result.response.ok || result.response.status === 409) {
        hideBanner(true);
        await refreshBots(true);
      } else {
        var message =
          "Could not " +
          verb +
          " " +
          name +
          ": " +
          errorMessage(result.data, result.response);
        showBanner(
          message,
          function () {
            void mutateBot(dir, verb);
          },
          true,
        );
        await refreshBots(true);
      }
    } catch (error) {
      showBanner(
        "Could not " +
          verb +
          " " +
          name +
          ". Check the connection and try again.",
        function () {
          void mutateBot(dir, verb);
        },
        true,
      );
    } finally {
      ui.pending.delete(key);
      markCardBusy(card);
    }
  }

  async function changeAutostart(card, enabled) {
    if (!ui.auth || !card || !card.dir) {
      return;
    }
    var dir = card.dir;
    var key = opKey(dir, "autostart");
    if (ui.pending.has(key)) {
      return;
    }
    var name = card.bot ? card.bot.name : dir;
    ui.pending.add(key);
    ui.autostartIntent.set(dir, enabled);
    markCardBusy(card);
    try {
      var result = await api(
        "/api/bots/" + encodeURIComponent(dir) + "/autostart",
        {
          method: "PUT",
          body: JSON.stringify({ enabled: enabled }),
        },
      );
      if (result.response.status === 401) {
        showLogin("Your session expired. Sign in again.");
        return;
      }
      if (result.response.ok || result.response.status === 409) {
        if (result.response.status === 409) {
          ui.autostartIntent.delete(dir);
        }
        hideBanner(true);
        await refreshBots(true);
      } else {
        ui.autostartIntent.delete(dir);
        showBanner(
          "Could not update autostart for " +
            name +
            ": " +
            errorMessage(result.data, result.response),
          function () {
            var current = ui.cards.get(dir);
            if (current) {
              void changeAutostart(current, enabled);
            }
          },
          true,
        );
        await refreshBots(true);
      }
    } catch (error) {
      ui.autostartIntent.delete(dir);
      showBanner(
        "Could not update autostart for " +
          name +
          ". Check the connection and try again.",
        function () {
          var current = ui.cards.get(dir);
          if (current) {
            void changeAutostart(current, enabled);
          }
        },
        true,
      );
    } finally {
      ui.pending.delete(key);
      if (ui.cards.get(dir) === card && card.bot) {
        updateCard(card, card.bot);
      }
      markCardBusy(card);
    }
  }

  function bindSettings() {
    if (
      !els.settingsDialog ||
      !els.settingsForm ||
      !els.settingsButton ||
      !window.AnqaTheme
    ) {
      return;
    }

    function syncSettings() {
      var current = window.AnqaTheme.get();
      var theme = els.settingsForm.querySelector(
        'input[name="theme"][value="' + current.theme + '"]',
      );
      var accent = els.settingsForm.querySelector(
        'input[name="accent"][value="' + current.accent + '"]',
      );
      if (theme) {
        theme.checked = true;
      }
      if (accent) {
        accent.checked = true;
      }
    }

    els.settingsButton.addEventListener("click", function () {
      syncSettings();
      els.settingsDialog.showModal();
      var focused = els.settingsForm.querySelector(
        'input[name="theme"]:checked, input[name="accent"]:checked',
      );
      if (focused) {
        focused.focus();
      }
    });

    if (els.settingsClose) {
      els.settingsClose.addEventListener("click", function () {
        els.settingsDialog.close();
      });
    }

    els.settingsForm.addEventListener("change", function (event) {
      if (event.target.name === "theme" || event.target.name === "accent") {
        var theme = els.settingsForm.querySelector(
          'input[name="theme"]:checked',
        );
        var accent = els.settingsForm.querySelector(
          'input[name="accent"]:checked',
        );
        if (theme && accent) {
          window.AnqaTheme.set(theme.value, accent.value);
        }
      }
    });

    els.settingsDialog.addEventListener("click", function (event) {
      if (event.target === els.settingsDialog) {
        els.settingsDialog.close();
      }
    });
  }

  function bindEvents() {
    els.loginForm.addEventListener("submit", function (event) {
      void handleLogin(event);
    });
    els.tokenInput.addEventListener("input", hideLoginError);

    els.refreshButton.addEventListener("click", function () {
      void refreshBots();
    });

    els.logoutButton.addEventListener("click", function () {
      void handleLogout();
    });

    els.bannerRetry.addEventListener("click", function () {
      var retry = ui.bannerRetry;
      ui.bannerSticky = false;
      if (typeof retry === "function") {
        retry();
      } else {
        void refreshBots();
      }
    });

    document.addEventListener("visibilitychange", function () {
      if (!ui.auth) {
        return;
      }
      if (document.hidden) {
        stopPolling();
      } else {
        void refreshBots();
      }
    });

    bindSettings();
  }

  bindEvents();
  void bootstrap();
})();
