# CommuCat CLI Client 🐾

[![CI](https://github.com/ducheved/commucat-cli-client/actions/workflows/ci.yml/badge.svg)](https://github.com/ducheved/commucat-cli-client/actions/workflows/ci.yml)
[![Release](https://github.com/ducheved/commucat-cli-client/actions/workflows/release.yml/badge.svg)](https://github.com/ducheved/commucat-cli-client/actions/workflows/release.yml)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL--2.0-orange.svg)](LICENSE)

[![Website](https://img.shields.io/badge/commucat.tech-live-blue?logo=firefox)](https://commucat.tech)
[![Contact](https://img.shields.io/badge/Ducheved-me%40ducheved.ru-6f42c1?logo=minutemailer)](mailto:me@ducheved.ru)

> Терминальный клиент CCP-1 в стиле k9s: Noise-рукопожатие, HTTP/2, многооконный TUI. **Свободны как кошки!**

---

## 📚 Документация
- Быстрое знакомство → эта страница.
- Расширенное руководство → [`docs/CLIENT_GUIDE.md`](docs/CLIENT_GUIDE.md).
- Протокол CCP-1 → см. [docs/PROTOCOL.md в серверном репо](https://github.com/ducheved/commucat/blob/main/docs/PROTOCOL.md).

---

## ⚙️ Возможности
- Полное Noise XK/IK рукопожатие поверх HTTPS/HTTP2.
- Командный режим (`:connect`, `:join`, `:relay`, `:presence`, `:channel`, `:export`).
- Отображение ACK, presence, системных эвентов.
- Локальный профиль (`~/.config/commucat/client.json`) с ключами устройства.

## Требования
| Компонент | Версия | Примечание |
|-----------|--------|------------|
| Rust      | 1.75+  | для сборки из исходников |
| Сервер    | CommuCat 1.0+ | доступен по HTTPS |
| TLS       | Публичный CA или путь к self-signed CA (`--tls-ca`) |

---

## 🚀 Быстрый старт
```bash
git clone https://github.com/ducheved/commucat-cli-client.git
cd commucat-cli-client
cargo build --release
# бинарь: target/release/commucat-cli-client
```
Либо скачайте готовый архив из GitHub Releases (`commucat-cli-client-linux-amd64.tar.gz`).

### Инициализация профиля
```bash
cargo run -- init \
  --server https://chat.example:8443 \
  --domain chat.example \
  --tls-ca /path/to/server.crt   # для самоподписанных сертификатов
```
После инициализации зарегистрируйте ключ на сервере:
```bash
ssh commucat@chat.example \
  "source /opt/commucat/.env && commucat-cli rotate-keys device-123"
```
Файл `~/.config/commucat/client.json` содержит hex-ключи и настройки presence.

### Запуск TUI
```bash
cargo run -- tui
# или ./target/release/commucat-cli-client tui
```
Команды TUI перечислены в [`docs/CLIENT_GUIDE.md`](docs/CLIENT_GUIDE.md#команды-tui).

---

## 🔐 TLS
- Используйте `--tls-ca <path>` для самоподписанного сертификата сервера.
- Продакшен: CN/SAN серта должен совпадать с `--server`/`--domain`.
- Для локального теста допускается `--insecure`, но не используйте в проде.

## 🛠️ Диагностика
| Сообщение | Причина | Решение |
|-----------|---------|---------|
| `tcp connect failed` | Сервер доступен только по IPv4 | Используйте `https://127.0.0.1:8443` |
| `tls connect failed` | Хост ≠ CN/SAN или CA не доверен | `--tls-ca` или обновите сертификат |
| `handshake rejected` | Устройство не зарегистрировано | `commucat-cli rotate-keys <device-id>` |
| `dns lookup failed` | Ошибка в URL | Проверьте `client.json` |

---

## 🧰 Расширенные сценарии
- Изменение presence интервала → редактируйте `presence_interval_secs` в профиле.
- Ротация ключей → `commucat-cli-client init --force --device-id <id>` + повторная регистрация.
- HEADLESS режим (без TUI) — TODO (см. Roadmap).

Подробнее: [`docs/CLIENT_GUIDE.md`](docs/CLIENT_GUIDE.md).

---

## 🗺️ Roadmap
- Headless режим (скриптовое управление без TUI).
- Export/import профилей.
- Сборка под musl/ARM64.
- Интеграция с password vault для хранения ключей.

---

## Simple English Corner
Hello friend! Quick steps:
1. Build or download binary.
2. `commucat-cli-client init --server https://host --domain host [--tls-ca ca.pem]`.
3. Register device on server (`commucat-cli rotate-keys device-id`).
4. Run `commucat-cli-client tui`, type `:connect`, `:join ...`, then start chatting. Profile is stored at `~/.config/commucat/client.json`. Use `--insecure` only for local testing. Free like cats! 🐱
