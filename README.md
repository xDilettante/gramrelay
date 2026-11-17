# tg — прототип Telegram-библиотеки на Rust

Библиотека развивается до полного объёма [проекта архитектуры](docs/ARCHITECTURE.md). Crate находится в C:/dev/Rust/TelegramRust, версия 0.1.0-alpha.1. Имя tg рабочее; публикация отключена. Критерии завершения: [tasks/goal.md](tasks/goal.md).

Реализованы awaitable builders для send_message, answer_callback_query и send_document; inline-кнопки, разбор callback Update, типизированные ID, ограничения нагрузки и локальные HTTP mock-тесты.

## Подключение

```toml
[dependencies]
tg = { path = "C:/dev/Rust/TelegramRust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde_json = "1" # Для примера разбора callback Update
```

Токен передаётся через доверенное окружение TELEGRAM_BOT_TOKEN. Основной вызов:

```rust,no_run
use tg::{Bot, ChatId};

#[tokio::main]
async fn main() -> tg::Result<()> {
    let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
    let sent = bot
        .send_message(ChatId::from(-1001234567890), "Привет!")
        .disable_notification(true)
        .await?;
    println!("message_id={}", i64::from(sent.message_id));
    Ok(())
}
```

Запрос ленивый: создание builder не обращается к сети. Действие начинается при await. Повторное бизнес-действие — отдельный вызов, который библиотека не считает дубликатом автоматически.

## Callback

```rust,no_run
use tg::{Bot, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Update};

async fn example(bot: &Bot, received_json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let keyboard = InlineKeyboardMarkup::new()
        .row([InlineKeyboardButton::callback("Подтвердить", "confirm")]);
    bot.send_message(ChatId::from(123456789), "Продолжить?")
        .reply_markup(keyboard)
        .await?;

    let update: Update = serde_json::from_str(received_json)?;
    if let Some(query) = update.callback_query {
        bot.answer_callback_query(query.id)
            .text("Событие получено")
            .await?;
    }
    Ok(())
}
```

Хост проверяет право пользователя выполнять прикладное действие. Callback data не является доказательством такого права. Update можно передать вручную в Router::dispatch или получать через управляемый polling. Неизвестные поля, в том числе внутри Message, сохраняются; повреждённые известные объекты и два известных вида события в одном Update отклоняет декодер. Webhook подключается отдельно; доступны управляемые режимы с памятью и устойчивым SQLite-feed.

## Webhook-адаптер

Опция webhook включает tg::webhook::{WebhookSecret, WebhookIngress, WebhookOptions, WebhookSink}. Ingress проверяет единственный X-Telegram-Bot-Api-Secret-Token до чтения тела, резервирует квоту и разбирает полный Update. Пользовательский WebhookSink получает событие и исходный deadline; его успешный результат должен означать обработку или commit, а не запись в очередь памяти. Нельзя начинать новый deadline после ожидания тела/очереди.

WebhookIngress::router() встраивается в существующий Axum/TLS-сервер хоста. serve_until(listener, stop) запускает ограниченный HTTP/1-сервер: по умолчанию 64 соединения, 5 секунд на заголовки и 60 секунд жизни соединения. Он слушает переданный loopback listener; для доверенного TLS reverse proxy с другим адресом нужен явный trusted_proxy(true). По умолчанию путь /telegram, тело 1 MiB, 16 принятых запросов и 16 MiB резервирования, общий request_timeout 30 секунд. Остановка закрывает listener и admission, завершает принятые запросы в shutdown_timeout (30 секунд), затем отменяет и дожидается своих connection tasks. Удалённый webhook автоматически не устанавливается и не удаляется.

Этот низкоуровневый слой не проверяет getMe и не занимает источник: хост обязан проверить личность, удерживать Bot::claim_update_source(), координировать источник между процессами и выбрать дедупликацию. Bot::webhook объединяет процессное владение, Router и дедупликацию в памяти. UpdateFeed::webhook в tg-agent объединяет HTTP-адаптер с SQLite и возобновляемым source lease. Оба режима используют входящую диагностику tracing/metrics; разрешения на чтение и действия остаются частью A1.

Обычный бот с очередью и дедупликацией в памяти:

```rust,no_run
use tg::{Bot, CancellationToken, Router};
use tg::webhook::WebhookSecret;

async fn serve(
    bot: &Bot,
    listener: tokio::net::TcpListener,
    secret: WebhookSecret,
    stop: CancellationToken,
) -> tg::Result<()> {
    let router = Router::new().command("start", |ctx| async move {
        ctx.reply("Привет!").await?;
        Ok(())
    });
    bot.webhook(router, secret).serve_until(listener, stop).await
}
```

Создание builder не обращается к сети. serve_until проверяет getMe, привязывает username команд и занимает общий SDK source guard. MemoryWebhookOptions по умолчанию ограничивает ожидающие и активные события: 100 записей и 16 MiB канонического payload; DispatchOptions — 16 активных групп и 30 секунд на обработчик. Независимые чаты допускаются по мере поступления; события одного чата идут последовательно. Неизвестная область ждёт предыдущую работу и не пропускает последующие события.

Кэш хранит до 100 000 fingerprint/ID в течение 24 часов после успеха. Совпадающий успешный повтор получает 200, повтор в работе — 503, конфликтующий payload — 409. Действующие записи не вытесняются: заполнение даёт 503. Ошибка, panic или timeout не записываются как успех. Квота очереди удерживается до завершения работы даже после разрыва HTTP; просроченное событие в очереди освобождает её по исходному deadline. После stop.cancel() нужно дождаться serve_until: обработчики завершаются или отменяются с ожиданием до освобождения источника. При перезапуске память теряется; внешние эффекты требуют стабильных operation_id и подходящего хранилища.

Офлайн-пример использует настоящий localhost HTTP и синтетическую личность из test-kit:

```powershell
cargo run --locked --offline -p tg --features webhook,test-kit --example webhook_mock
```

## Polling и маршрутизация

Слой bot включён по умолчанию; клиент без него подключается через default-features = false.

```rust,no_run
use tg::{Bot, MessageContext, Router};

async fn start(ctx: MessageContext) -> tg::Result<()> {
    ctx.reply("Привет!").await?;
    Ok(())
}

#[tokio::main]
async fn main() -> tg::Result<()> {
    let bot = Bot::from_env("TELEGRAM_BOT_TOKEN")?;
    bot.run_polling(Router::new().command("start", start)).await
}
```

Router::with_state передаёт обработчикам явное общее состояние. Доступны command, on_message, callback, on_callback и on_update; выполняется первый совпавший маршрут. Ответ MessageContext::reply сохраняет chat_id, message_id, тему и business connection.

Polling проверяет getMe и getWebhookInfo, не удаляет существующий webhook и разрешает один управляемый poller на bot ID внутри процесса. Ручные вызовы get_updates и другие процессы требуют координации приложения.

По умолчанию пачка содержит до 100 событий, одновременно работают до 16 групп чатов; внутри чата порядок последовательный. При неизвестной области события вся пачка обрабатывается последовательно. Следующий offset отправляется только после успешного завершения всей пачки. Ошибка, паника или тайм-аут обработчика останавливает polling без подтверждения этой пачки. После перезапуска уже выполненные действия могут повториться: постоянный журнал ещё не интегрирован.

run_polling обрабатывает Ctrl-C и завершает принятую работу в пределах shutdown timeout. Для встраивания в приложение используйте run_polling_until с PollingOptions и CancellationToken: этот вариант не регистрирует обработчик сигналов. Серверный poll_timeout должен быть меньше общего deadline клиента. Первые значения по умолчанию — 20 секунд для long polling и 30 секунд для обработчика/остановки. Транспортные ошибки возвращаются вызывающему коду; автоматического перезапуска пока нет.

## Файлы

```rust,no_run
use tg::{Bot, ChatId, InputFile};

async fn example(bot: &Bot, chat: ChatId) -> tg::Result<()> {
    let sent = bot
        .send_document(chat, InputFile::path("report.pdf"))
        .caption("Отчёт")
        .await?;

    if let Some(document) = sent.document {
        bot.send_document(chat, InputFile::file_id(document.file_id.as_str()))
            .await?;
    }
    Ok(())
}
```

InputFile::bytes поддерживает уже имеющийся буфер. InputFile::path открывает обычный файл при выполнении запроса и передаёт его потоком, без чтения всего файла в RAM. Поток ограничен размером, полученным из открытого файла; файл следует сохранять неизменным до завершения отправки. Локальные пути предназначены для доверенного кода приложения, а не для непосредственного ввода модели.

## Ошибки и тайм-ауты

По умолчанию builder делает одну попытку; внутренние повторы reqwest отключены. У сгенерированных запросов есть явная RetryPolicy для проверенных безопасных случаев.

| Состояние ошибки | Значение |
|---|---|
| NotSent | Запрос не передан HTTP-клиенту: локальная валидация, ошибка подготовки файла, перегрузка или deadline до dispatch |
| Rejected | Получен корректный однозначный отказ Bot API |
| Unknown | Эффект мог произойти: тайм-аут после dispatch, транспортный сбой, повреждённый ответ, 5xx или превышение лимита ответа |

```rust,no_run
use tg::{Bot, ChatId, EffectOutcome};

async fn example(bot: &Bot, chat: ChatId) -> tg::Result<()> {
    match bot.send_message(chat, "Отчёт").await {
        Ok(sent) => println!("message_id={}", i64::from(sent.message_id)),
        Err(error) if error.outcome() == EffectOutcome::Unknown => {
            // Сохранить неопределённую попытку в приложении; не отправлять заново вслепую.
            return Err(error);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}
```

retry_after и migrate_to_chat_id доступны через Error. Без RetryPolicy они не запускают повтор; миграция никогда не меняет адресата автоматически. Ошибки сохраняют категорию и числовые коды, но не содержат сырых URL, текстов запросов, серверных descriptions и цепочек ошибок транспорта с токеном.

Внешняя отмена future не возвращает Error и не отменяет уже выполненное действие Telegram. Если future уже выполнялась, приложение должно считать её результат неизвестным. Постоянный журнал операций, подавление повторных бизнес-действий и гарантия exactly-once в этот прототип не входят.

## Настройки

RichText собирает entities без HTML-экранирования переменных и считает UTF-16 offsets:

```rust
use tg::{EntityKind, RichText};
let text = RichText::new("Отчёт: ")
    .append(RichText::styled("готов 🦀", EntityKind::Bold)?);
// bot.send_message(chat, text).await?;
# Ok::<(), tg::Error>(())
```

entity(range, kind) принимает байтовый диапазон Rust-строки; try_from_entities проверяет готовые UTF-16 spans. Helper проверяет границы, вложенность и metadata; его Debug скрывает текст. text()/entities()/into_parts() позволяют использовать результат в api::methods. Для явной разметки есть parse_mode(ParseMode::Html/MarkdownV2/Markdown); смешивание parse_mode и entities даёт локальную ошибку. Длину отрисованной разметки проверяет Telegram. Это отдельный helper от дерева api::types::RichText для sendRichMessage.

on_message_event(MessageEvent, handler) подключает business/edited/channel/guest-сообщения явно; polling-подписки задаются отдельно. ctx.reply сохраняет chat/thread/business/direct-message topic и эфемерного получателя. Для callback есть reply() с проверкой доступности сообщения и ephemeral_reply() по ID callback; answer() вызывается отдельно. Guest-сообщения требуют answer_guest(). Ошибки контекста не превращаются в обычную отправку в другой чат. Короткоживущие ephemeral ID проходят проверку 15-секундного окна по UTC-часам компьютера перед отправкой; доставка остаётся решением Telegram.

ctx.reply_document(InputFile::path(...)) сохраняет тот же контекст для документа. У callback также есть reply_document() и ephemeral_reply_document(). Файл открывается после проверки контекста при await; оставшееся время ephemeral-ответа ограничивает подготовку, upload и чтение ответа.

Bot::builder(token) поддерживает:

| Настройка | Значение по умолчанию |
|---|---|
| timeout | 30 секунд на подготовку, upload и чтение ответа |
| max_upload_bytes | 50 000 000 байт суммарно для новых файлов запроса |
| max_download_bytes | 20 000 000 байт на скачивание |
| max_attachments | 50 файлов на запрос |
| max_response_bytes | 1 MiB, проверяется также без Content-Length |
| max_in_flight | 32; превышение возвращает Capacity без очереди |
| api_url | https://api.telegram.org/ |
| redirects и повторы | Отключены |
| системные и environment proxy | Отключены |

Это начальные настройки прототипа, а не результаты нагрузочного подбора. Локальные лимиты не гарантируют принятие запроса сервером Telegram. TLS проверяет сертификаты. Переопределение api_url — доверенная настройка; HTTP разрешается отдельным флагом только для literal loopback IP в mock-тестах. Не включайте подробное wire logging транспорта с настоящим токеном.

## Закреплённая схема API

В schema/bot-api.json сохранена структура Bot API 10.4: 193 метода, 425 типов и SHA-256 официального источника. Все методы доступны через bot.api(), структуры запросов — в api::methods, полные wire-типы — в api::types, каталог — в api::catalog. Корневые Message/Update/User/Chat и результаты удобных методов используют эти же полные типы. Необязательные объекты и варианты union используют Box для ограничения размера значений; конструкторы, setters и From принимают обычные значения. Изменения API прототипа перечислены в [руководстве миграции](docs/MIGRATION.md).

```rust
use tg::{Bot, ChatId, api::methods};

async fn full_api(bot: &Bot) -> tg::Result<()> {
    let sent = bot.api().send_message("Привет!".into())
        .chat_id(ChatId::from(123456789).into())
        .protect_content(true)
        .await?;
    let request = methods::GetChat::new(sent.chat.id.into());
    bot.call(request).await?;
    Ok(())
}
```

Сборка запроса не отправляет его. Оба способа используют тот же транспорт, ограничения и семантику Unknown. schema/method-policy.json закрепляет эффекты всех методов за точным хешем схемы; метаданные не предоставляют агенту разрешений. Возвращаемые токены и поля с credentials используют SecretString: Debug скрыт, чтение требует expose_secret(). Сериализация SecretString намеренно сохраняет значение для отправки в API.

Wire InputFile — строковая ссылка на file ID, URL или attach://name. Новые файлы привязываются к сгенерированному запросу через attach; пути открываются только при await:

```rust
use tg::{Bot, ChatId, InputFile};

async fn upload(bot: &Bot) -> tg::Result<()> {
    bot.api().send_photo("attach://image".into())
        .chat_id(ChatId::from(123456789).into())
        .attach("image", InputFile::path("photo.jpg"))
        .await?;
    Ok(())
}
```

Это работает и для вложенных media group и rich messages. Имя привязки содержит до 64 ASCII-букв/цифр/подчёркиваний, начинается с буквы и отличается от имён параметров. Лимит max_attachments по умолчанию равен 50; лишние, повторные и отсутствующие привязки отклоняются до отправки. Общий размер ограничен max_upload_bytes. Ссылки распознаются только в типизированных файловых полях; обычный текст attach://… остаётся текстом. String-поля Telegram с файловой семантикой закреплены в schema/file-policy.json. Существующий file ID указывается прямо в wire-параметре, не через attach.

Скачивание использует File из getFile:

```rust
let file = bot.api().get_file(file_id).await?;
let bytes = bot.download_file(&file).max_bytes(1_000_000).bytes().await?;
// Для потока: bot.download_file(&file).to_writer(&mut writer).await?;
```

max_bytes только уменьшает общий max_download_bytes. Проверяются заявленный размер и фактически прочитанные chunks. Endpoint строится под доверенным api_url; некорректные пути не становятся URL или локальными файлами. to_writer не создаёт/не закрывает/не flush-ит назначение; при ошибке или отмене в нём может остаться префикс. Для bytes частичный успешный результат не возвращается. Автоматических повторов нет.

Проверенные чтения можно повторять явно:

```rust
let me = bot.api().get_me()
    .retry_policy(tg::RetryPolicy::default().max_attempts(3))
    .await?;
```

Политика ограничивает общее время (по умолчанию 30 секунд), попытки (3) и exponential backoff (100 ms…5 s) с jitter. retry_after не сокращается. Локальные ошибки, миграции, постоянные ошибки, credential/session-операции и Unknown-записи не повторяются. retry_rejected_writes(true) отдельно разрешает повтор обычной JSON-записи только после подтверждённого Bot API 429; короткоживущие query/ephemeral-ссылки исключены. Для upload-потоков такая политика возвращает retry_attachments до открытия файлов. Backoff не удерживает in-flight permit; отмена не оставляет фоновых попыток. Для воспроизводимых проверок есть jitter(false).

Инструмент разработки tools/schema.py использует стандартную библиотеку Python 3 и не обращается к сети. Сгенерированные файлы хранятся в Git; обычной сборке Cargo Python не нужен. Проверка и регенерация:

```powershell
python -m unittest discover -s tools/tests -v
python tools/schema.py generate
python tools/schema.py check
```

Для обновления схемы разработчик отдельно сохраняет официальную страницу в target/schema-cache/api.html и запускает python tools/schema.py import --source target/schema-cache/api.html. Изменение версии или количества определений требует явного пересмотра schema/expected.json. Файл schema/overrides.json содержит проверенные исключения; ненужные исключения и неизвестные типы вызывают ошибку.

## Постоянный журнал операций

Пакет tg-agent добавляет SQLite-журнал отдельно от зависимостей обычного клиента. Он атомарно резервирует ключи, связывает владельца и параметры, сохраняет результаты и после сбоя переводит незавершённую попытку в unknown. Для истёкшего или удалённого тела результата ключ не становится новым. Состояния и версии проверяются между отдельными процессами; действие начинается только после commit in_flight.

```powershell
cargo run --locked --offline -p tg-agent --example journal
```

Пример использует тестовый транспорт и проверяет две попытки обращения к журналу с одной отправкой. [Контракт и настройки tg-agent](tg-agent/README.md), [спецификация J1](SPEC-journal.md). Опция executor объединяет восемь действий с текущими grant/approval, долговечными квотами, областью bot/chat/file и безопасными квитанциями. Общий JSON-слой, CLI с run/serve и MCP stdio по конфигурации хоста реализованы; [руководство MCP](docs/AGENT-MCP.md) описывает запуск, лимиты и восстановление; доступ к хранилищу сам по себе не выдаёт разрешения Telegram. Офлайн-примеры authorized_send, authorized_read, authorized_callback и authorized_document показывают общий исполнитель без реальной сети.

## Устойчивый приём и подписчики

UpdateFeed в tg-agent сохраняет события, локальные курсоры, дедупликацию и независимые позиции подписчиков в SQLite. Приём polling подтверждает Telegram только после commit всего пакета. Подписчик отдельно подтверждает успешную обработку; после сбоя неподтверждённая пачка читается снова. Аренды ограничивают владельцев между процессами, а продление во время обработки прекращается при остановке или потере владения. Для внешних эффектов обработчика нужны стабильные operation_id: общей транзакции с Telegram нет.

Опция bot у tg-agent включает run_polling_until и run_consumer_until. Хранилище само по себе её не включает. Управляемые источники разделяют SDK guard по ID бота; прямые вызовы API требуют координации хоста. Общий Bot::dispatch_batch также доступен для своих источников с ограничениями DispatchOptions. [Контракт J2](SPEC-update-journal.md) описывает хранение, тайм-ауты и восстановление.

```powershell
cargo run --locked --offline -p tg-agent --features bot --example durable_bot
```

## Тесты без сети

Опция test-kit предоставляет tg::test_kit::{MockTransport, Step, MockResponse}. Пример запускается с синтетическим токеном в памяти:

```powershell
cargo run --locked --offline --features test-kit --example test_kit
```

MockTransport::new([...]) задаёт последовательность ожидаемых методов. Step::reply принимает ответ с envelope Bot API; Step::timeout, transport_error и partial_upload моделируют неопределённый исход. MockResponse поддерживает HTTP-статус, задержку заголовков/chunks, повреждённые ответы и прерывание потока. bot() создаёт обычный Bot, bot_builder() позволяет настроить его реальные тайм-ауты и лимиты. Непредусмотренный метод или исчерпанный сценарий возвращает ошибку; сетевого fallback нет.

requests() возвращает снимок фактически сериализованных запросов. decode::<api::methods::SendMessage>() проверяет имя метода и декодирует строгие JSON-параметры; body() позволяет проверить настоящий multipart и частично прочитанный upload. Debug скрывает данные fixtures и записей. Используйте синтетические данные: явное чтение body/decode намеренно раскрывает содержимое теста.

По умолчанию сценарий и история ограничены 256 запросами, общие записанные тела — 1 MiB; отдельный такой же лимит действует на тела ответов сценария. with_limits меняет эти доверенные настройки. assert_finished() проверяет, что шаги использованы, а потоки завершены или отменены. В обработчике можно использовать тот же Router::dispatch с подготовленными Update. При ошибках и отмене действуют общие правила Unknown и освобождается semaphore.

## Диагностика

Опции tracing и metrics подключаются отдельно и выключены по умолчанию. Приложение устанавливает собственный subscriber/recorder; библиотека не меняет глобальную конфигурацию. Target tg создаёт span tg.request с локальным attempt_id и событие request_finished с method, result, outcome, error_kind и duration_seconds. Span связан с текущим span приложения. Токены, URL, ID чатов, тексты и файлы не включаются.

Webhook POST-pipeline создаёт span tg.webhook с attempt_id и событие webhook_finished с фиксированными result/status/reason и duration_seconds. Метрики tg.webhook.requests и tg.webhook.duration_seconds используют только эти ограниченные категории; tg.webhook.active считает текущие pipeline до решения/отмены. accepted означает решение после обработки/commit, а не доказательство доставки ответа по сети. Отмена pipeline учитывается отдельно и освобождает gauge. В поля и метки не включаются URL/route, peer, bot/chat/update IDs, секреты и содержимое. Глобальный panic hook и диагностика внешнего TLS-сервера остаются настройкой хоста.

Метрики tg.request.attempts, tg.request.duration_seconds и tg.request.active позволяют считать ошибки/отмены, строить перцентили и наблюдать активные попытки. Метки ограничены именами методов и фиксированными категориями; attempt_id не используется как метка. Каждая повторная попытка учитывается отдельно. Отмена после отправки имеет outcome=unknown; перегрузка — not_sent. Локальная валидация до общего пути исполнения не создаёт попытку. Exporter и histogram buckets выбирает приложение.

## Примеры и проверка

Из корня проекта:

```powershell
cargo run --locked --offline --example mock_send
cargo test --locked --all-targets
cargo test --locked --doc
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo doc --locked --no-deps
cargo audit
```

Первый пример запускает только локальный mock и не требует токена. Параметр --offline предполагает, что зависимости уже загружены. Остальные примеры выполняют реальные запросы при явном запуске в подготовленном окружении:

```powershell
cargo run --example send_message -- 123456789 "Привет!"
cargo run --example upload_document -- 123456789 "C:/path/report.pdf"
cargo run --example answer_callback -- "C:/path/update.json"
cargo run --example polling
```

Проверки выполнялись на Windows x86_64, Rust 1.98.0: 246 unit/integration-тестов workspace по умолчанию, 410 со всеми опциями (включая 193 проверки tg-agent, 67 проверок tg-mtproto и 32 проверки webhook), 6 doctest и 25 тестов инструментов. Патчи backend отдельно проходят 59 тестов протокольного ядра, 3 теста TL-примитивов, 24 TL doctest и 12 тестов sender (16 с тестовым RPC-слоем). Clippy и сборки без default features проходят. Офлайн-примеры test_kit, journal, update_store, durable_bot, authorized_send, authorized_read, authorized_callback, authorized_document и agent_json запускаются без сокетов; webhook_mock и durable_webhook используют только localhost и синтетическую личность; отдельная матрица tg проверена ранее на 106/119/89 тестах для стандартной, полной и клиентской конфигураций. Структурные fixtures охватывают все 425 типов и 193 метода, включая обязательные поля, запрет неизвестных параметров запроса и chat scope каждого известного события. MSRV и работа на Linux/macOS пока не подтверждены. Проверяются JSON/multipart, callback, файлы, локальные отказы, 429, 5xx, неизвестные ответы, тайм-ауты, ограничения нагрузки, redirects, секреты в диагностике, маршрутизация, polling, порядок событий и остановка. MCP дополнительно проверен через официальный SDK и настоящий stdio-процесс; офлайн-архив доступен для десяти заданий оценки, успешность LLM пока не измерена. SDK не обращался к живому Telegram в этих проверках.

Для отдельного MTProto-клиента выбран grammers 0.12.0 после офлайн-прототипов grammers и TDLib. [Решение и границы проверки](docs/decisions/0016-mtproto-backend.md) фиксируют обязательные доработки. Реализованы зашифрованная Session и сетевой Client с типизированным raw TL, ограничениями, тайм-аутами и согласованной остановкой. Строгий входной разбор закреплён отдельными проверяемыми патчами SDK. Доступны вход бота/пользователя по коду и 2FA, проверка self-личности, ограниченная миграция DC и выход с постоянным карантином. [Прикладной API MTProto](tg-mtproto/README.md) поддерживает поиск собеседников, непрозрачные ссылки из зашифрованного кеша и отправку/редактирование текста с проверкой аккаунта перед действием. Диалоги, история, файлы, восстановление обновлений и интеграция агента ещё разрабатываются; сборка MTProto пока требует workspace с этими патчами.

Пакет [tg-mtproto](tg-mtproto/README.md) уже содержит зашифрованное хранилище сессий с арендой владельца, fencing, отдельным подтверждённым checkpoint и безопасным завершением native-работ. Подготовлен узкий [патч grammers-mtsender](vendor/grammers-mtsender/PATCH.md): остановка прерывает установку соединения, дожидается дочерних задач и сохраняет сигнал потери updates вне заполненной очереди. Эти этапы проверены на синтетических данных и localhost; сетевой Client, авторизация и восстановление updates ещё реализуются.

Следующие этапы: отдельный MTProto-клиент и выпускные проверки, включая измерения производительности и оценку удобства для агентов. Полная цель остаётся активной. Источники контрактов: [docs/SOURCES.md](docs/SOURCES.md). План: [tasks/todo.md](tasks/todo.md).
