# CommuCat CLI Client Guide

Подробное руководство по терминальному клиенту CCP-1. Краткая выжимка доступна в `README.md`.

## Содержание
- [Назначение и архитектура](#назначение-и-архитектура)
- [Установка](#установка)
- [Настройка профиля](#настройка-профиля)
- [Команды TUI](#команды-tui)
- [TLS и доверие](#tls-и-доверие)
- [Диагностика](#диагностика)
- [Troubleshooting](#troubleshooting)
- [Расширенные сценарии](#расширенные-сценарии)
- [Roadmap](#roadmap)

---

## Назначение и архитектура

Компоненты клиента:
- **Transport**: HTTPS/HTTP2 соединение с сервером CommuCat. Клиент перебирает все адреса, возвращённые `lookup_host`, поддерживает IPv4/IPv6.
- **Noise XK/IK**: приложение создаёт Noise-ручку (`commucat_crypto::build_handshake`) и проводит трёхходовой обмен. Сервер проверяет статический ключ устройства, но не читает payload.
- **CCP-1**: после Noise перехода каждое сообщение упаковывается в кадр (HELLO, AUTH, MSG, JOIN и т.д.). Сервер просто маршрутизирует.
- **TUI**: построен на `ratatui` + `crossterm`. Левый столбец — список каналов, правая панель — сообщения/ивенты, нижняя строка — ввод команд/сообщений. Горячие клавиши максимально близки к k9s (Tab — следующий канал, Ctrl+C/F10 — выход).

Основной поток:
1. Загрузка профиля (`client.json`) → создание Noise конфигурации.
2. `:connect` → TCP → TLS → Noise → ACK — при успехе TUI показывает `session` и начинает слушать стрим.
3. Дальше пользователь может `:join` каналы, отправлять сообщения, менять presence. Сервер рассылает ACK/Presence/MSG кадры, которые отображаются в правой панели.

### Преимущества/ограничения
- Клиент не хранит историю сообщений — только текущее окно. Исторические кадры следует получать от сервера (relay_queue).
- В TUI нет шифрования на клиенте — предполагается, что приложение доходит до Noise транспортного канала, а payload уже шифруется внешними приложениями (в текущей версии клиент работает как транспорт).

---

## Установка

### Сборка из исходников
```bash
git clone https://github.com/ducheved/commucat-cli-client.git
cd commucat-cli-client
cargo build --release
# бинарь появится в target/release/commucat-cli-client
```

### Использование готовых бинарей
1. Перейдите в раздел Releases репозитория.
2. Скачайте архив `commucat-cli-client-linux-amd64.tar.gz`.
3. Распакуйте и, при необходимости, добавьте бинарь в `$PATH`:
   ```bash
   tar -xzf commucat-cli-client-linux-amd64.tar.gz -C /usr/local/bin
   chmod +x /usr/local/bin/commucat-cli-client
   ```

### Проверка
```bash
commucat-cli-client --help
```
Если выводится список команд (`init`, `export`, `docs`, `tui`), сборка прошла успешно.

---

## Настройка профиля

Профиль — JSON (`client.json`) в каталоге `~/.config/commucat` (можно переопределить через `COMMUCAT_CLIENT_HOME`). Структура:
```json
{
  "device_id": "device-123",
  "server_url": "https://chat.example:8443",
  "domain": "chat.example",
  "private_key": "...hex32...",
  "public_key": "...hex32...",
  "noise_pattern": "XK",
  "prologue": "commucat",
  "tls_ca_path": "/path/to/server.crt",
  "server_static": null,
  "insecure": false,
  "presence_state": "online",
  "presence_interval_secs": 30,
  "traceparent": null
}
```

### Создание нового профиля
```bash
commucat-cli-client init \
  --server https://chat.example:8443 \
  --domain chat.example \
  --tls-ca /path/to/server.crt \
  --pattern XK \
  --presence online
```
Параметры:
- `--device-id` — явно задать ID (иначе сгенерируется `device-<timestamp>`).
- `--server-static` — hex публичного ключа сервера (нужно для Noise IK).
- `--insecure` — отключить проверку TLS (только для локального стенда!).
- `--force` — перезаписать существующий профиль.

### Регистрация ключа на сервере
После `init` в выводе появятся `device_id`, `public_key`, `private_key`. На сервере выполните:
```bash
source /opt/commucat/.env
commucat-cli rotate-keys device-123
```
или добавьте запись в `user_device` вручную (hex публичного ключа).

### Изменение параметров
- Редактируйте JSON вручную или используйте `init --force`.
- Текущее состояние presence/interval можно менять из TUI (`:presence`), файл обновится автоматически.

---

## Команды TUI

### Горячие клавиши
| Клавиша | Действие |
|---------|----------|
| `:`     | перейти в режим команд (ввод начинается с `:`) |
| `Tab`   | переключить активный канал вперёд |
| `Shift+Tab` (не реализовано) | TODO |
| `Esc`   | очистить строку ввода |
| `Enter` | отправить сообщение/команду |
| `Ctrl+C` / `F10` | выход |

### Команды (вводятся после `:`)
| Команда | Описание |
|---------|----------|
| `connect` | установить TLS/Noise сессию |
| `disconnect` | закрыть текущую сессию |
| `join <channel> <member1,member2,...>` | объявить канал, список устройств (себя можно не указывать — добавится автоматически) |
| `relay <channel> <members>` | join с `relay=true` (сервер делает фан-аут) |
| `leave <channel>` | покинуть канал |
| `channel <channel>` | переключить активный канал в TUI |
| `presence <state>` | обновить presence (сохраняется в профиле) |
| `export` | вывести ключи устройства (public/private) |
| `clear` | очистить историю сообщений в TUI |
| `help` | краткая справка |
| `quit` / `exit` | завершить программу |

Если ввод не начинается с `:`, строка считается сообщением и отправляется в активный канал (`FrameType::Msg`).

---

## TLS и доверие

### Публичные сертификаты
Если сервер использует сертификат от доверенного CA (Let’s Encrypt, ZeroSSL), дополнительных действий не требуется.

### Самоподписанные серты
- Передайте `--tls-ca /path/to/server.crt` при `init`. Файл должен содержать полный цепочку (root → server) или хотя бы self-signed корень.
- Убедитесь, что `server_url`/`domain` совпадают с CN/SAN. Если сертификат на `localhost`, используйте `https://localhost:8443`.

### Инспекция ошибок
- `tls connect failed`: уточнение ошибки выводится в логе TUI (`decode error`, `invalid certificate`). Проверяйте hostname и путь к CA.
- Для отладки можно временно использовать `--insecure`, но это отключает проверку цепочки — не рекомендуется за пределами локальной машины.

---

## Диагностика

### Логи TUI
- События отображаются в правой панели.
- Для отладки включите `RUST_LOG=debug` перед запуском (`RUST_LOG=debug commucat-cli-client tui`).

### Проверка профиля
```bash
cat ~/.config/commucat/client.json | jq
```
Проверьте, что hex-строки корректные, `server_url` и `domain` совпадают.

### Проверка доступности сервера
```bash
curl -k https://chat.example:8443/healthz
```
`200 OK` означает, что сервер онлайн.

---

## Troubleshooting

| Симптом | Возможная причина | Что делать |
|---------|-------------------|------------|
| `tcp connect failed` | Сервер слушает только IPv4; либо порт недоступен | Используйте `https://127.0.0.1:8443`; проверьте firewall/port |
| `connect attempt [::1] failed` | IPv6 loopback закрыт | Это ожидаемо, клиент перейдёт на следующий адрес |
| `tls connect failed` | hostname ≠ CN/SAN, CA не доверен | Добавьте `--tls-ca`, выпустите корректный сертификат |
| `handshake rejected` | Устройство не зарегистрировано / неверный public key | Проверьте таблицу `user_device`, повторно выполните `rotate-keys` |
| `unexpected frame` | Сервер прислал неизвестный тип | Проверить совместимость версий 
| Нет сообщений в канале | Канал не создан, нет `join` | Выполните `:join <channel> self,peer` |

---

## Расширенные сценарии

### Изменение каталога профиля
```bash
export COMMUCAT_CLIENT_HOME=/etc/commucat-client
commucat-cli-client init --force --server ...
```
Файл `client.json` появится в указанной директории.

### Скриптовое использование
- Сейчас реализован только TUI. В roadmap — headless режим (`commucat-cli-client send ...`).
- Можно использовать `cargo run -- docs --lang en` для вывода README (англ/рус).

### Ротация ключей
```bash
commucat-cli-client init --force --device-id device-2025 \
  --server https://chat.example:8443 --domain chat.example --tls-ca /path/to/ca
# затем обновите запись на сервере (rotate-keys)
```
После ротации старые сообщения в очередях остаются валидными (сервер не читает payload).

---

## Roadmap
- Headless API (отправка/приём сообщений без TUI).
- Плагинный слой для кастомных декодеров payload.
- Встроенная проверка сертификатов с системными хранилищами (macOS Keychain, Windows Cert Store).
- Сборка под ARM64/Linux и Windows (MSYS2).
- Автодетект обновлений и авто-апдейт профиля.

---

_Последнее обновление: 2025-02._
