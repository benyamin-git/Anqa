# Echo bot

A small Telegram bot that echoes text messages. It uses only the Python
standard library, so the image has no third-party dependencies.

## Run it

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy its token.
2. Copy this directory into the Anqa bots directory:

   ```sh
   cp -r examples/echo-bot /path/to/anqa/bots/echo-bot
   ```

3. Create the environment file and add the token:

   ```sh
   cp /path/to/anqa/bots/echo-bot/.env.example /path/to/anqa/bots/echo-bot/.env
   $EDITOR /path/to/anqa/bots/echo-bot/.env
   ```

4. Open the Anqa dashboard and press **Start** on **echo-bot**. Anqa builds the
   Dockerfile on the first start and reuses the Docker build cache after that.

Do not commit `.env`; it contains the bot token. Anqa loads `.env`
automatically when it exists.

## How it works

The bot long-polls Telegram's `getUpdates` method and sends each text message
back to the same chat with `sendMessage`. Network errors and API rate limits
use exponential backoff. Invalid tokens stop the process with a clear error.

The source is [bot.py](bot.py).
