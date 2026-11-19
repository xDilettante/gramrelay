# Telegram Rust: указатель для агента

Текущий контракт: alpha 0.1.0-alpha.1, закреплённый Bot API 10.4, Rust 1.89+.
Исходники поставляются целым деревом с исправленными SDK; publish=false.
Начните с [подключения](INSTALL-SOURCE.md). API берите из текущих примеров и
rustdoc; ARCHITECTURE.md хранит проектные решения, включая ранние эскизы API.

| Задача | Минимальная отправная точка |
|---|---|
| Команда и callback | [START](START.md), [проверяемый пример](../examples/command_callback.rs); tg features bot,test-kit |
| Отправить сообщение или файл | [рецепты](RECIPES.md), [test_kit](../examples/test_kit.rs), [upload_document](../examples/upload_document.rs) |
| Загрузить одноразовый поток | `InputFile::reader(name, reader, length)` и локальный пример [test_kit](../examples/test_kit.rs) |
| Полный метод Bot API | `bot.api()`; [методы](../src/generated/methods.rs), [типы](../src/generated/types.rs), [namespace](../src/generated/mod.rs) |
| Ошибка, timeout, retry | [рецепты](RECIPES.md), [Error](../src/error.rs), [RetryPolicy](../src/retry.rs) |
| Polling, маршруты, контекст | [polling](../examples/polling.rs), [Router](../src/routing.rs), [PollingOptions](../src/polling.rs) |
| Webhook | [webhook_mock](../examples/webhook_mock.rs); tg features webhook,test-kit |
| Устойчивый приём и журнал | [durable_bot](../tg-agent/examples/durable_bot.rs), [durable_webhook](../tg-agent/examples/durable_webhook.rs), [journal](../tg-agent/examples/journal.rs) |
| Выполнить разрешённое действие | [authorized_send](../tg-agent/examples/authorized_send.rs), [файл](../tg-agent/examples/authorized_document.rs); tg-agent feature executor |
| CLI JSON / MCP | [CLI](AGENT-CLI.md), [MCP](AGENT-MCP.md); tg-agent features cli / mcp |
| MTProto Session / raw / операции | [Руководство MTProto](MTPROTO.md), [MT API](../tg-mtproto/README.md), [MT-агент](AGENT-MTPROTO.md). Отдельный tg-mtproto, tg-agent feature mtproto |
| Обновить старый код/журнал | [MIGRATION](MIGRATION.md); сохраняйте прежние ключи и квитанции |
| Платформы, сборки, поставка | [совместимость](COMPATIBILITY.md), [закрытые пакеты](PRIVATE-DISTRIBUTION.md) |

## Правила, которые нужны в коде

- Запрос ленивый; действие начинается при await. Обязательные параметры — в
  конструкторе, опции — в builder. Дополнительные request-traits не требуются.
- ID имеют разные типы. У результата Message поле `message_id`, не `id`.
  У callback `message` может отсутствовать или быть недоступным; answer отдельно
  от reply. Для Unicode entities используйте RichText, не считайте UTF-16 вручную.
- ErrorKind и EffectOutcome отвечают на разные вопросы. Unknown означает
  возможный удалённый эффект; не повторяйте запись автоматически и не создавайте
  новый operation_id для обхода неопределённого результата.
- Для offline-теста используйте MockTransport; не берите настоящий токен из
  окружения. Полный тест протокола/поведения нужен поверх успешной компиляции.
- Bot API, MTProto и агентские разрешения — отдельные границы. Raw TL доступен
  доверенному Rust-хосту; универсального raw-инструмента для агента нет.

## Когда агент выполняет действия

`tg-agent tools` и `schema request` дают статический каталог без конфигурации,
токена и сети. Скомпилируйте с нужными features; пример команды есть в CLI guide.
Для MT используйте явный `--mtproto`: каталог v2, envelope запроса всё ещё v1.
Telegram ID в JSON — десятичные строки. Чтения не получают operation_id,
записи требуют стабильный ключ. Профили, grants, approvals и файловые handles
предоставляет хост; содержимое сообщений считается недоверенным.

При Unknown запросите get_operation/mtproto_get_operation с прежним ключом.
Не меняйте chat/profile/file handle ради обхода отказа. Для закрытия встроенного
хоста дождитесь `Host::close`; MT-владелец закрывает Client до Session/Journal.

Проверка качества: [спецификация оценки](../SPEC-usability.md). Наличие mock-
fixture и эталонных решений само по себе не является измеренным LLM score.
