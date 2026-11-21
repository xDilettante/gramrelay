# История изменений

## Не выпущено

### Добавлено

- `PollingOptions::reconnect` включает восстановление `getUpdates` с прежним `offset`, задержкой с jitter и учётом `retry_after`. Отмена прерывает ожидание. `tracing` добавляет событие `polling_reconnect`. Стартовые `getMe` и `getWebhookInfo`, `401` и `409`, `ResponseTooLarge` и ошибки обработчиков завершают цикл.
- `BotBuilder::retain_remote_descriptions(true)` открывает явный доступ к `Error::untrusted_remote_description()`. Описание извлекается из ответов с ошибкой, ограничивается по длине UTF-8 и очищается от управляющих символов и токена, включая форму с `%3A`. Стандартные ошибки, метрики и квитанции `tg-agent` сохраняют безопасное представление.
- Модуль `rich_message` (`RichDoc`, `RichSpan`, `RichTable`, `RichTableCell`, `RichList`, `RichButton`, `HeadingLevel`, `TextAlign`, `VerticalAlign`, `ButtonStyle`) с локальной валидацией в `RichDoc::build() -> Result<InputRichMessage>` и `TryFrom<RichDoc>`.
- Алиас `tg::EntityText = tg::RichText` для разграничения с `tg::api::types::RichText`.
- Контекстные и прямые Rich-ответы на общем `Delivery` и общем multipart-пути: `Bot::send_rich_message`, `Bot::reply_ephemeral_rich`, `MessageContext::reply_rich`, `CallbackContext::reply_rich`, `CallbackContext::ephemeral_reply_rich`.
- Расширенные корневые `InlineKeyboardButton` (`callback`, `url`, `copy_text`, `style`) и `InlineKeyboardMarkup` на базе сгенерированных типов `api::types` с конверсиями `From` в обе стороны и локальной валидацией.
- Маршрутизация параметризованных callback через `Router::callback_prefix` с приоритетом точного маршрута над префиксным.

### Исправлено

- `WebhookIngress::serve_until` продолжает работу после временного сбоя `listener.accept()`. При нехватке ресурсов следующий приём откладывается. Отмена прерывает паузу, фатальная ошибка завершает сервер.

### Проверено

- На Windows x64 прошли 723 теста рабочего пространства со всеми возможностями, 17 doctests и 67 целевых тестов на Rust 1.89.0. Clippy с `-D warnings`, форматирование и `cargo audit` прошли.

## [v0.1.0-alpha.1] — 2026-11-20

Первый альфа-релиз GramRelay. Минимальная поддерживаемая версия Rust — **1.89.0**.

### Добавлено

- `tg` — типизированный клиент Telegram Bot API с отправкой сообщений, inline-кнопками и callback, загрузкой и скачиванием файлов, polling и webhook. Для локальных тестов доступен mock-транспорт.
- `tg-mtproto` — отдельный клиент MTProto с зашифрованным хранилищем сессий, отправкой сообщений и файлов, чтением истории и приёмом обновлений с явным подтверждением обработки.
- `tg-agent` — JSON CLI и MCP по stdio для Bot API и MTProto. Хост задаёт права на действия, а SQLite-журнал хранит квитанции операций для восстановления результата по `operation_id`.
- Проверенный ZIP с исходниками, JSON-манифест и файл контрольных сумм `SHA256SUMS` в GitHub Releases.

### Исправлено перед выпуском

- Проверка зависимостей в архиве корректно разрешает пути к одному каталогу на Windows и macOS.
- Тесты тайм-аутов MTProto используют виртуальные часы. Сценарий зависшей отправки отдельно проверяет исход `Unknown`, отсутствие повторной записи и завершение обработчика.
- Тест SQLite-журнала проверяет истечение аренды и срока операции после подтверждённого начала ожидания блокировки записи.

### Проверено

На ревизии `a5170db` прошли Windows x64, Linux x64 и macOS ARM64 с Rust 1.89.0 и stable. В каждой конфигурации проверен внешний потребитель из распакованного архива. Аудит зависимостей завершился успешно. [Прогон Compatibility](https://github.com/xDilettante/gramrelay/actions/runs/44639167741)

[v0.1.0-alpha.1]: https://github.com/xDilettante/gramrelay/releases/tag/v0.1.0-alpha.1
