#!/usr/bin/env python3
"""A tiny Telegram echo bot using only the Python standard library."""

import json
import logging
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API_ROOT = "https://api.telegram.org"
LOG = logging.getLogger("echo-bot")


class TelegramError(Exception):
    """A failed Telegram Bot API call."""

    def __init__(self, description, code=None, retry_after=None):
        super().__init__(description)
        self.description = description
        self.code = code
        self.retry_after = retry_after


def call(token, method, params=None, timeout=30):
    """Call one Bot API method and return its result."""
    url = f"{API_ROOT}/bot{token}/{method}"
    data = None
    if params is not None:
        data = urllib.parse.urlencode(params).encode("utf-8")
    request = urllib.request.Request(url, data=data)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as error:
        try:
            payload = json.load(error)
        except (ValueError, OSError):
            payload = {}
        description = payload.get("description") or error.reason or "request failed"
        raise TelegramError(str(description), error.code) from None

    if not payload.get("ok"):
        parameters = payload.get("parameters") or {}
        raise TelegramError(
            payload.get("description") or "request failed",
            payload.get("error_code"),
            parameters.get("retry_after"),
        )
    return payload.get("result")


def clamp_text(text, limit=4096):
    """Trim text to Telegram's UTF-16 code-unit limit."""
    if len(text.encode("utf-16-le")) <= limit * 2:
        return text
    while text and len(text.encode("utf-16-le")) > (limit - 1) * 2:
        text = text[:-1]
    return text + "…"


def send_message(token, chat_id, text, message_thread_id=None):
    params = {
        "chat_id": chat_id,
        "text": clamp_text(text),
        "disable_web_page_preview": "true",
    }
    if message_thread_id is not None:
        params["message_thread_id"] = message_thread_id
    call(token, "sendMessage", params, timeout=15)


def handle_update(token, update):
    message = update.get("message")
    if not isinstance(message, dict):
        return
    text = message.get("text")
    chat = message.get("chat") or {}
    if not isinstance(text, str) or not text:
        return
    try:
        send_message(token, chat.get("id"), text, message.get("message_thread_id"))
    except TelegramError as error:
        LOG.warning("sendMessage failed: %s", error.description)
        if error.retry_after:
            time.sleep(min(float(error.retry_after), 30.0))
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        LOG.warning("sendMessage failed: %s", error)


def poll(token):
    offset = 0
    backoff = 1.0
    while True:
        try:
            updates = call(
                token,
                "getUpdates",
                {
                    "timeout": 30,
                    "offset": offset,
                    "allowed_updates": json.dumps(["message"]),
                },
                timeout=40,
            )
            backoff = 1.0
            for update in updates:
                update_id = update.get("update_id")
                if isinstance(update_id, int) and update_id >= offset:
                    offset = update_id + 1
                handle_update(token, update)
        except TelegramError as error:
            if error.code in (401, 404):
                LOG.error("Telegram rejected the bot token: %s", error.description)
                return 1
            if error.retry_after:
                delay = min(float(error.retry_after), 60.0)
            else:
                delay = backoff
            LOG.warning(
                "getUpdates failed: %s; retrying in %.0fs",
                error.description,
                delay,
            )
            time.sleep(delay)
            backoff = min(backoff * 2, 60.0)
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            LOG.warning("Cannot reach Telegram: %s; retrying in %.0fs", error, backoff)
            time.sleep(backoff)
            backoff = min(backoff * 2, 60.0)
        except KeyboardInterrupt:
            return 0


def main():
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
    )
    token = os.environ.get("TELEGRAM_BOT_TOKEN", "").strip()
    if not token:
        LOG.error("TELEGRAM_BOT_TOKEN is not set")
        return 1
    LOG.info("Echo bot is polling for messages")
    return poll(token)


if __name__ == "__main__":
    sys.exit(main())
