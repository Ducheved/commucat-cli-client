# CommuCat CLI Client 🐾

[![CI](https://github.com/ducheved/commucat-cli-client/actions/workflows/ci.yml/badge.svg)](https://github.com/ducheved/commucat-cli-client/actions/workflows/ci.yml)
[![Release](https://github.com/ducheved/commucat-cli-client/actions/workflows/release.yml/badge.svg)](https://github.com/ducheved/commucat-cli-client/actions/workflows/release.yml)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL--2.0-orange.svg)](LICENSE)
[![Website](https://img.shields.io/badge/commucat.tech-live-blue?logo=firefox)](https://commucat.tech)
[![Contact](https://img.shields.io/badge/Ducheved-me%40ducheved.ru-6f42c1?logo=minutemailer)](mailto:me@ducheved.ru)

> Терминальный клиент CCP-1 с Noise-рукопожатием, HTTP/2-туннелем и многооконным TUI. **Свободны как кошки!**

---

## Обзор
CommuCat CLI Client подключается к серверу CommuCat, выполняет Noise XK/IK рукопожатие, ведёт учёт устройств профиля и предоставляет удобный TUI в стиле k9s. Клиент ориентирован на операторов/пауэр-юзеров: за секунды можно создать профиль, выпустить pairing-код, синхронизировать список друзей и запросить рекомендации P2P assist.

- Шифрование и аутентификация: Noise handshake + device certificate с проверкой CA.
- Многооконный интерфейс: вкладки для чатов, устройств, друзей, pairing, server info и P2P assist.
- REST-мост: операции `/api/pair`, `/api/devices`, `/api/friends`, `/api/p2p/assist` доступны из TUI и CLI.
- Конфигурация хранится в `client.json`, автоматическое обновление `user_id`, сертификата и session token после рукопожатия.
- Работает на Windows, Linux, macOS (Rust async + Crossterm/Ratatui).

## Возможности
- Полноценное Noise XK/IK рукопожатие поверх HTTPS/HTTP2 с поддержкой `device_ca_public` и ZKP-доказательства.
- Многооконный TUI c горячими клавишами (F1–F6) и командной строкой `:`.
- Управление профилем: генерация pairing-кодов, приём новых устройств, просмотр/отзыв устройств, синхронизация друзей.
- Инспекция сервера: вкладка с `/api/server/info` и контроль P2P assist (`/api/p2p/assist`).
- CLI-команды для автоматизации (`init`, `pair`, `claim`, `devices`, `friends`, `export`, `tui`).
- Возможность работы в мульти-девайсной схеме: одно `user_id`, несколько `device_id`.

## Требования
| Компонент | Версия | Примечание |
|-----------|--------|------------|
| Rust      | 1.75+  | для сборки клиента из исходников |
| Сервер    | CommuCat 1.0+ | HTTPS, включает REST `/api/*` |
| TLS       | Публичный CA или путь к self-signed CA (`--tls-ca`), для отладки допустим `--insecure` |
## Установка
### Из исходников
```bash
### Из исходников

```bash
git clone https://github.com/ducheved/commucat-cli-client.git
cd commucat-cli-client
> - Windows (Chocolatey + MSYS2/WSL): `choco install vpx` и убедитесь, что `pkg-config` видит `lib/pkgconfig/vpx.pc`. Можно задать `setx PKG_CONFIG_PATH "C:\ProgramData\chocolatey\lib\vpx\tools\pkgconfig"`
>

### Готовые релизы

2. Скачайте архив `commucat-cli-client-<platform>.tar.gz`.
3. Распакуйте и добавьте бинарь в `$PATH` (пример для Linux):

   ```bash
   tar -xzf commucat-cli-client-linux-amd64.tar.gz -C /usr/local/bin
   chmod +x /usr/local/bin/commucat-cli-client

   ```

### Проверка

```bash
commucat-cli-client --help
```
Должны отобразиться команды `init`, `pair`, `claim`, `devices`, `friends`, `docs`, `tui`.
---
## Быстрый старт

1. **Создайте профиль устройства.**

   ```bash
   commucat-cli-client init \
     --server https://chat.example:8443 \
     --domain chat.example \
     --username alice \
     --tls-ca /etc/ssl/certs/chat-example.pem
   ```
   Профиль сохранится в `~/.config/commucat/client.json` (или `$COMMUCAT_CLIENT_HOME`).

2. **Получите pairing-код для новых устройств (опционально).**
   ```bash
   commucat-cli-client pair --ttl 900 --session <rest-session-token>
   ```
   Код и seed сохранятся в `client.json` (`last_pairing_code`).

3. **Подключите второе устройство.**
   ```bash
   commucat-cli-client claim ABCD-EFGH --device-name "Laptop"
   ```
   Команда скачает новый ключ, сертификат и обновит профиль.

4. **Запустите TUI и подключитесь.**
   ```bash
   commucat-cli-client tui
   ```
   Введите `:connect` — после успешного рукопожатия в статусной строке появится `session=<uuid>` и `user=<id>`. `user_id` и сертификат автоматически сохранятся в `client.json`.

---

## TUI навигация
| Клавиша | Раздел | Что отображается |
|---------|--------|------------------|
| F1      | Chat   | Каналы, события, ACK/MSG, ввод сообщений |
| F2      | Devices | Список устройств, статусы, revoke/inspect (`r`, `v`, `i`) |
| F3      | Friends | Друзья, алиасы, pull/push (`r`, `p`, `d`) |
| F4      | Pairing | Текущий pairing-код, выдача нового (`g`) |
| F5      | Info    | `/api/server/info`: версии, noise_static, auto-approve |
| F6      | Assist  | Отчёт `/api/p2p/assist`, обновление (`r`) |
| Tab/Shift+Tab | — | Переключение каналов (в Chat) или вкладок |
| Enter   | — | В не-чат вкладках показывает детали записи |
| Ctrl+C / F10 | — | Выход из приложения |

Командная строка (начинается с `:`):
- `:connect`, `:disconnect`
- `:join <channel> <members>` / `:relay <channel> <members>`
- `:leave <channel>` / `:channel <id>`
- `:presence <state>`
- `:pair [ttl]`
- `:devices list|revoke <device_id>`
- `:friends list|add <user_id> [alias]|remove <user_id>|push|pull`
- `:export`, `:clear`, `:help`, `:quit`

---

## Конфигурация профиля
Путь по умолчанию: `~/.config/commucat/client.json` (на Windows `%APPDATA%\commucat\client.json`). Измените через `COMMUCAT_CLIENT_HOME`.

Ключевые поля:
- `device_id`, `private_key`, `public_key` — текущая пара ключей устройства (hex).
- `server_url`, `domain`, `noise_pattern`, `prologue`, `server_static`, `tls_ca_path`, `insecure`.
- `user_handle`, `user_display_name`, `user_avatar_url` — предпочтения профиля.
- `user_id` — устанавливается сервером после первого успешного рукопожатия или `claim`.
- `session_token` — REST токен; используется TUI/CLI при работе с `/api/*`.
- `device_certificate*`, `device_ca_public` — сохранённый сертификат устройства и CA.
- `friends` — локальный список друзей (с алиасами), синхронизируется через `:friends push/pull`.

Любые изменения в файле применяются после перезапуска клиента или `:connect`.

---

## CLI команды
| Команда | Пример | Назначение |
|---------|--------|------------|
| `commucat-cli-client init` | `--server https://chat.example:8443 --domain chat.example --username alice` | Создание/обновление профиля устройства |
| `commucat-cli-client pair` | `--ttl 900 --session <token>` | Запрос pairing-кода через REST |
| `commucat-cli-client claim` | `ABCD-EFGH --device-name Laptop` | Получение ключей и сертификата нового устройства |
| `commucat-cli-client devices list` | `--session <token>` | Список устройств пользователя |
| `commucat-cli-client devices revoke` | `<device-id> --session <token>` | Перевод устройства в состояние `revoked` |
| `commucat-cli-client friends add` | `<user-id> --alias Bob --push` | Управление списком друзей и синхронизация с сервером |
| `commucat-cli-client export` | — | Вывод текущей пары ключей |
| `commucat-cli-client docs` | `--lang en` | Печать руководства (RU/EN) |
| `commucat-cli-client tui` | — | Запуск интерактивного интерфейса |

---

## Как написать собственного клиента
1. **Получите доверенный TLS и регистрационные данные.** Сгенерируйте устройство (`commucat-cli rotate-keys` на сервере) или используйте pairing-код.
2. **Установите TLS 1.3 соединение** с `POST /connect` (HTTP/2 предпочтительно, поддерживается HTTP/1.1 chunked).
3. **Сформируйте Noise конфигурацию** (XK или IK) и отправьте `HELLO` (`FrameType::Hello`):
   - JSON-поля: `protocol_version`, `pattern`, `device_id`, `client_static`, `device_public`, `handshake` (Noise message 1), `capabilities`, `zkp`.
   - Опционально: `certificate` (pre-issued DeviceCertificate), `user` (`handle`, `display_name`, `avatar_url`, `id`), `device_ca_public`.
4. **Примите `AUTH`** (Noise message 2). Расшифруйте payload — сервер вернёт `session`, `domain`, `protocol_version`, `user_id`, снимок профиля и (при необходимости) обновлённый сертификат.
5. **Отправьте завершающий `AUTH`** (Noise message 3) и ждите `ACK` с `handshake: "ok"`. Сохраняйте `session`, `user`, `device_certificate`, `device_ca_public`.
6. **Маршрутизируйте кадры CCP-1**: используйте `JOIN/LEAVE/MSG/ACK/VOICE_FRAME/VIDEO_FRAME` согласно [PROTOCOL.md](https://github.com/ducheved/commucat/blob/main/PROTOCOL.md). Сервер требует монотонных `sequence` и varint-кодирование длин.
7. **Работайте с REST API** для полноценных возможностей: `/api/pair`, `/api/pair/claim`, `/api/devices`, `/api/friends`, `/api/p2p/assist`, `/api/server/info`. Аутентификация — header `Authorization: Bearer <session>`.
8. **Сохраняйте состояние**: persist `client.json`-аналог с ключами, `user_id`, `session_token`, `friends`, `device_certificate`. Перегенерируйте Noise ключи при ротации сертификата.

В качестве примера реализации смотрите `src/engine.rs` (Noise + поток) и `src/rest.rs` (REST клиент) этого репозитория.

---

## Диагностика
| Сообщение | Причина | Решение |
|-----------|---------|---------|
| `tcp connect failed` | Сервер недоступен, IPv6-адрес закрыт | Проверьте URL, попробуйте `https://127.0.0.1:8443` |
| `tls connect failed` | CN/SAN ≠ hostname или CA незнаком | Укажите `--tls-ca`, проверьте сертификат |
| `handshake rejected` | Устройство не зарегистрировано / превышен лимит auto-approve | Выпустите сертификат (`pair`/`claim` или `rotate-keys`), проверьте лимиты |
| `unexpected frame` | Несовместимые версии CCP-1 | Обновите клиент/сервер, сравните `supported_versions` |
| `REST 401/403` | Неверный `session_token` | Выполните `:connect` или обновите токен через `init --session` |

Включите `RUST_LOG=debug` для подробных логов (`RUST_LOG=debug commucat-cli-client tui`).

---

## Roadmap
- Headless режим (`commucat-cli-client send` / `receive`) для скриптов без TUI.
- История сообщений и экспорт в файлы.
- Поддержка аппаратных ключей (Secure Element) для хранения private key.
- ОС-специфичные хранилища TLS (macOS Keychain, Windows Cert Store).
- Интеграция с UI уведомлениями (desktop notifications).

---

## Simple English Corner
1. Build or download the binary.
2. `commucat-cli-client init --server https://chat.example --domain chat.example [--tls-ca ca.pem]` to create `client.json`.
3. Optional: `commucat-cli-client pair --ttl 900 --session <token>` and `commucat-cli-client claim CODE --device-name Laptop` for extra devices.
4. Run `commucat-cli-client tui`, type `:connect`, join channels with `:join <channel> <members>`, enjoy multi-tab TUI. Profile and `user_id` stay in `~/.config/commucat/client.json`. Use `--insecure` only for local labs.
