FROM python:3.13-alpine

WORKDIR /app
COPY bot.py .

USER 65534:65534
CMD ["python", "-u", "bot.py"]
