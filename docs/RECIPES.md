# Рецепты

Все команды выполняются из полного исходного дерева. Для обычного Bot API
достаточно `tg`; `tg-agent` добавляет журнал/политику/инструменты, `tg-mtproto`
используется отдельно. [Подключение и кэш зависимостей](INSTALL-SOURCE.md).

| Хочу сделать | Проверенный исходник | Что выбрать |
|---|---|---|
| Команду с кнопкой | [command_callback](../examples/command_callback.rs) | Default offline; --live включает polling с подготовленным env |
| Прямую отправку | [send_message](../examples/send_message.rs) | Bot::send_message, результат Message.message_id |
| Тест без сокетов | [test_kit](../examples/test_kit.rs) | features test-kit; очередь Step, проверка записи и assert_finished |
| Документ с диска | [upload_document](../examples/upload_document.rs) | InputFile::path; живой пример только при явном запуске |
| Документ из AsyncRead | [test_kit](../examples/test_kit.rs) | InputFile::reader с известной длиной отправляет указанный префикс потока |
| Документ по host handle | [authorized_document](../tg-agent/examples/authorized_document.rs) | Хост фиксирует байты, scope и срок; агент не передаёт произвольный путь |
| Ответить на callback | [answer_callback](../examples/answer_callback.rs) | ID из Update; answer подтверждает нажатие, прикладные права отдельно |
| Polling | [polling](../examples/polling.rs) | Для критичного приёма добавьте долговечный feed |
| Webhook локально | [webhook_mock](../examples/webhook_mock.rs) | features webhook,test-kit; аутентификация секрета и bounded body |
| Пережить дубликаты updates | [durable_bot](../tg-agent/examples/durable_bot.rs), [durable_webhook](../tg-agent/examples/durable_webhook.rs) | Явный ACK, стабильный consumer и настройка retention |
| Пережить повтор команды агента | [authorized_send](../tg-agent/examples/authorized_send.rs) | Стабильный operation_id и текущий grant, одна отправка |
| Читать через инструменты | [authorized_read](../tg-agent/examples/authorized_read.rs), [agent_json](../tg-agent/examples/agent_json.rs) | IDs строками, профиль задан хостом, чтение без operation_id |
| Подключить MCP | [MCP guide](AGENT-MCP.md), [mcp_fixture](../tg-agent/examples/mcp_fixture.rs) | Реальный stdio fixture без Telegram; LLM-оценка отдельно |
| MT текст/callback/файл | [text](../tg-agent/examples/mtproto_authorized_text.rs), [callback](../tg-agent/examples/mtproto_authorized_callback.rs), [document](../tg-agent/examples/mtproto_authorized_document.rs) | tg-agent feature mtproto-test-kit для offline-примеров; подготовленные сессии — [MT host](AGENT-MTPROTO.md) |

## Ошибка и повтор

`InputFile::reader("report.bin", reader, length)` принимает принадлежащий
приложению `AsyncRead + Send + 'static`. Источник читается при исполнении
запроса. Если он короче указанной длины, запрос завершается ошибкой. Остаток
длинного потока остаётся непрочитанным. Длина входит в общий лимит вложений.
Раннее подтверждение при неполной загрузке даёт `Unknown`.

В примере `test_kit` поток проверяется локально вместе с обычным сообщением.

```powershell
cargo run --locked --offline -p tg --features test-kit --example test_kit
```

| EffectOutcome | Значение для вызывающего кода |
|---|---|
| NotSent | Локальная отправка не состоялась. Устраните причину и проверьте журнал/политику перед новым решением |
| Rejected | Есть подтверждённый отказ; прочитайте безопасный kind/code и допустимый retry_after |
| Unknown | Сервер мог выполнить действие. Сохраните ключ/квитанцию; не запускайте новую запись автоматически |

Запрос делает одну попытку по умолчанию. RetryPolicy предназначен для
проверенных чтений; подтверждённая обычная JSON 429-запись требует отдельного
opt-in. Unknown-записи, миграции, временные ссылки и multipart не становятся
повторяемыми из-за установки политики. После частичной загрузки в writer
download не начинает всё заново скрытно.

Тайм-аут или отмена будущего не доказывают, что запись не ушла. Для агента
проверка прежнего operation_id и неизменных profile/target/file scopes важнее
попытки «исправить» результат новым ключом. [Миграции журнала](MIGRATION.md)
сохраняют эти границы; откат к pre-send backup не является способом повтора.

## Ограничить работу и остановиться

Настройте ограничения клиента и PollingOptions до запуска. Параллелизм,
размер очереди/партии, тайм-аут обработчика и серверная квота решают разные
задачи. Новые виды событий подписываются явно через allowed_updates; маршрут
сам не расширяет polling-подписку. Для наблюдения включите tracing/metrics
и не записывайте тела, токены или аргументы SDK в диагностические цепочки.

У встроенного агентского Host дождитесь close, сохраняя его до завершения.
У отдельного MT Client сначала дождитесь Client::close; он удерживает capacity
для выполняющейся работы и присоединяет драйвер до закрытия Session.
Примеры с нормальным завершением не заменяют проверки отмены и перезапуска.

## Перенести существующий Bot API-код

Сопоставляйте запрос Telegram, тип результата и его scope, а не только имя
метода другой библиотеки. Короткие методы и `bot.api()` используют общую
проверку отправки. Переход к builders не сохраняет чужую политику повторов
автоматически: явно перенесите лимиты, правила Unknown, подписки и ACK.
Старые prototype DTO/контексты и схемы SQLite описаны в [MIGRATION](MIGRATION.md).
