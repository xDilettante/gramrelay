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
    println!("message_id={}", i64::from(sent.id));
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

Хост проверяет право пользователя выполнять прикладное действие. Callback data не является доказательством такого права. Update можно передать вручную в Router::dispatch или получать через управляемый polling. Неизвестные поля, в том числе внутри Message, сохраняются; повреждённые известные объекты и два известных вида события в одном Update отклоняет декодер. Webhook endpoint ещё не реализован.

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

Все автоматические повторы отключены, в том числе внутренние повторы reqwest. Один выполненный builder делает одну попытку.

| Состояние ошибки | Значение |
|---|---|
| NotSent | Запрос не передан HTTP-клиенту: локальная валидация, ошибка подготовки файла, перегрузка или deadline до dispatch |
| Rejected | Получен корректный однозначный отказ Bot API |
| Unknown | Эффект мог произойти: тайм-аут после dispatch, транспортный сбой, повреждённый ответ, 5xx или превышение лимита ответа |

```rust,no_run
use tg::{Bot, ChatId, EffectOutcome};

async fn example(bot: &Bot, chat: ChatId) -> tg::Result<()> {
    match bot.send_message(chat, "Отчёт").await {
        Ok(sent) => println!("message_id={}", i64::from(sent.id)),
        Err(error) if error.outcome() == EffectOutcome::Unknown => {
            // Сохранить неопределённую попытку в приложении; не отправлять заново вслепую.
            return Err(error);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}
```

retry_after и migrate_to_chat_id доступны через Error как параметры для решения приложения. Они не запускают повтор и не меняют адресата автоматически. Ошибки сохраняют категорию и числовые коды, но не содержат сырых URL, текстов запросов, серверных descriptions и цепочек ошибок транспорта с токеном.

Внешняя отмена future не возвращает Error и не отменяет уже выполненное действие Telegram. Если future уже выполнялась, приложение должно считать её результат неизвестным. Постоянный журнал операций, подавление повторных бизнес-действий и гарантия exactly-once в этот прототип не входят.

## Настройки

Bot::builder(token) поддерживает:

| Настройка | Значение по умолчанию |
|---|---|
| timeout | 30 секунд на подготовку, upload и чтение ответа |
| max_upload_bytes | 50 000 000 байт для нового файла |
| max_response_bytes | 1 MiB, проверяется также без Content-Length |
| max_in_flight | 32; превышение возвращает Capacity без очереди |
| api_url | https://api.telegram.org/ |
| redirects и повторы | Отключены |
| системные и environment proxy | Отключены |

Это начальные настройки прототипа, а не результаты нагрузочного подбора. Локальные лимиты не гарантируют принятие запроса сервером Telegram. TLS проверяет сертификаты. Переопределение api_url — доверенная настройка; HTTP разрешается отдельным флагом только для literal loopback IP в mock-тестах. Не включайте подробное wire logging транспорта с настоящим токеном.

## Закреплённая схема API

В schema/bot-api.json сохранена структура Bot API 10.4: 193 метода, 425 типов и SHA-256 официального источника. Каталог доступен в api::catalog. Полный вызываемый Rust API поверх этих данных ещё находится в работе.

Инструмент разработки tools/schema.py использует стандартную библиотеку Python 3 и не обращается к сети. Сгенерированные файлы хранятся в Git; обычной сборке Cargo Python не нужен. Проверка и регенерация:

```powershell
python -m unittest discover -s tools/tests -v
python tools/schema.py generate
python tools/schema.py check
```

Для обновления схемы разработчик отдельно сохраняет официальную страницу в target/schema-cache/api.html и запускает python tools/schema.py import --source target/schema-cache/api.html. Изменение версии или количества определений требует явного пересмотра schema/expected.json. Файл schema/overrides.json содержит проверенные исключения; ненужные исключения и неизвестные типы вызывают ошибку.

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

Проверки выполнялись на Windows x86_64, Rust 1.98.0: 43 unit/integration-теста и 2 doctest, а также 10 тестов инструментов схемы; проходят также тесты клиентской сборки без слоя bot. MSRV и работа на Linux/macOS пока не подтверждены. Проверяются JSON/multipart, callback, файлы, локальные отказы, 429, 5xx, неизвестные ответы, тайм-ауты, ограничения нагрузки, redirects, секреты в диагностике, маршрутизация, polling, порядок событий и остановка. Живые вызовы Telegram при разработке не выполнялись.

Следующие этапы: полная схема Bot API, постоянный журнал и webhook endpoint, CLI/MCP, затем отдельный MTProto-клиент. Это частичное покрытие Bot API; полная цель остаётся активной. Источники контрактов: [docs/SOURCES.md](docs/SOURCES.md). План: [tasks/todo.md](tasks/todo.md).
