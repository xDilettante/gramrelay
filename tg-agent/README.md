# tg-agent

Модуль `profiles` хранит ожидаемый вид аккаунта отдельно от его числового ID:
`ProfileIdentity::bot_api`, `mtproto_bot`, `mtproto_user`. Хост вызывает
`journal.bind_profile(name, identity).await?`; точное повторное открытие допустимо,
подмена вида/ID и алиас того же аккаунта отклоняются. `profile_identity` читает
эти метаданные. Привязка не проверяет вход и не выдаёт разрешение Telegram.

Миграция журнала v6 сохраняет существующие feeds как Bot API; неизвестный аккаунт
для старой операции или квоты не угадывается. `UpdateFeed::open` сохраняет свою
привязку атомарно с созданием feed и не принимает имя MTProto-профиля. Новых
привязок по умолчанию не более 64 (`JournalOptions::max_profiles`, максимум 1024),
без удаления прежних ключей. Bot API CLI/MCP продолжают использовать v1;
расширенный каталог MTProto выбирается явно. Самостоятельная загрузка MTProto
профилей доступна через защищённую конфигурацию Host v2.

Если старые операции или квоты не имели feed и известной идентичности, попытка
назначить этому имени аккаунт получает `Conflict`. Их ручной журнал сохраняется;
смена имени или новый ключ не устанавливают исход прежнего `Unknown`.

Опция `mtproto` открывает отдельные `mtproto::Request`, `Grant`, `PeerId` и
`PeerScope`. Восемь запросов имеют префикс `mtproto_`, собеседник задаётся
объектом с `kind: user/chat/channel` и строковым bare ID. Типы не принимают
Bot API chat/thread options, access hashes, пути или произвольный raw TL.
`PolicyStore::replace_mtproto`, `dry_run_mtproto` и `approve_mtproto` используют
тот же principal/profile и ревизии, что Bot grants; смешения разрешений нет.
Подтверждение связано со всем неизменяемым запросом и operation_id. Проверка
политики локальная: она не резервирует квоту и не проверяет вход. Исполнение
доступно через отдельный Rust `mtproto::Executor` и встроенные CLI/MCP адаптеры
с явно подключённым провайдером либо Host v2 в самостоятельном процессе;
каталог Bot API содержит прежние восемь инструментов.

`mtproto::Executor` принимает `register_profile(feed, client)` от хоста и выполняет
восемь действий: get_me, get_peer, read_updates, send_message, edit_message_text,
answer_callback_query, get_operation и send_document с префиксом `mtproto_`. `get_peer` проверяет личность и доступность
собеседника в зашифрованном кеше; подготовку username/cache выполняет хост.
`Executor::ACTIONS` показывает фактически исполняемый набор. Вход/выход из аккаунта и raw TL не
являются действиями агента.

Миграция v8 добавляет отдельные точные MTProto scopes в общий журнал ключей и
квот. Отправка/редактирование проходят prepare → текущие права/квоты и commit
in_flight → M2 с абсолютным дедлайном → постоянная квитанция. `Unknown` и
`Rejected` не отправляются повторно; cached/get_operation не требуют RPC и
сохраняют исходную область доступа. `dry_run`/`approve` не расходуют квоту и не
читают тело сохранённой квитанции. Текущие права проверяются и после получения
ответа из owned-задачи, чтобы буферизация не обходила отзыв.

По умолчанию — 16 принятых jobs, 1 MiB сохранённых данных на чтение и 5 секунд
receipt margin; timeout берётся у M2 Client. Timeout + margin должны помещаться
в аренду журнала. Отмена вызывающего кода оставляет принятую задачу и её слот
до записи результата. Хост закрывает `executor.close().await`, затем Client,
затем Journal. Отменённый close можно продолжить; Drop не обещает завершённую
квитанцию или joined shutdown. Контракт: [SPEC-mtproto-agent-actions.md](../SPEC-mtproto-agent-actions.md).

Офлайн-пример с синтетической зашифрованной сессией и настоящим TL-кодеком:
`cargo run --locked --offline -p tg-agent --features mtproto-test-kit --example mtproto_authorized_text`.
Он проверяет две попытки с одним ключом и одну отправку без сокетов.

Файлы хост регистрирует через `register_file(principal, profile, filename, bytes,
expires_at_ms)`; агент получает только handle и отдельное право на него. Снимок
неизменяем: `restore_file` принимает лишь прежние размер/хеш и владельца, а
`discard_file`/`prune_files` освобождают резидентные ссылки. Активная загрузка
удерживает свой снимок и permit, включая отмену вызывающего кода. По умолчанию
FileLimits: 50 MiB на файл/активные загрузки, 100 MiB и 1024 снимка на Executor;
учитывается фактическая ёмкость Vec до hash/metadata ожиданий. Байты не копируются
во второй полный буфер M2. Пустые MT документы и небезопасные имена отклоняются;
MIME фиксирован как `application/octet-stream`. Пути, URL и file IDs не являются
аргументами инструмента.

Свежая отправка проверяет current peer/file/action grants, точный approval,
резидентные байты, expiry и квоты; полная длина файла учитывается атомарно с
допуском записи. Абсолютный дедлайн ограничивает cache identity и всю загрузку.
Квитанция содержит лишь peer/message ID. Cached/get_operation читаются после
discard, expiry и рестарта при текущих исходных правах, без SDK/RPC или байтов.
`Unknown`/`Rejected` не повторяются с прежним ключом. Подтверждённый `NotSent`
можно продолжить с тем же намерением после точного восстановления.

Миграция v9 сохраняет метаданные и прежние handles, заменяя Bot-only FK файлов
привязкой к общим account_profiles. Старые сироты и противоречивые виды аккаунта
отклоняются с полным откатом; байты после рестарта восстанавливает хост.
Сначала закрывается Executor (jobs/квитанции), затем Client (SDK/native cleanup),
затем Journal. Контракт: [SPEC-mtproto-agent-files.md](../SPEC-mtproto-agent-files.md).

`cargo run --locked --offline -p tg-agent --features mtproto-test-kit --example mtproto_authorized_document`
проверяет настоящие TL-части и сохранённую квитанцию без сокетов.

Ответ callback принимает только непрозрачный `reference`, текст и `show_alert`.
`read_updates` добавляет `callback_reference` к разрешённому событию; доверенный
хост также может вызвать `observe_callback(principal, profile, stored_event)`.
Нужен MT bot и исходное событие того же экземпляра Journal/профиля. Область
определяется по чату сообщения, а inline требует явного `unscoped_updates`.
Ссылка не выдаёт права ответа и не заменяет approval. Query ID и access hash
никогда не входят в инструментальные аргументы или квитанции.

По умолчанию TTL — 10 секунд от исходного приёма (1 мс..60 секунд), реестр —
1024 ссылки (до 65536), `CallbackClockPolicy::JournalMonotonic` требует исходного
монотонного наблюдения. Рестарт теряет это RAM-доказательство. Явный host-only
`TrustedWallClock` разрешает восстановление по исходному UTC; хост отвечает за
часы. Повторные чтения и новые алиасы не продлевают TTL. Общая для копий отметка
времени не позволяет оживить уже истёкшую ссылку откатом часов.

Дедлайн ограничивает ожидание identity и ответ; только TL Bool true подтверждает
успех. Квитанция содержит лишь `acknowledged=true`. Admission query общий для всех
principals/алиасов в одном профиле одной БД: после допуска другой ключ запрещён,
включая `Unknown`, `Rejected` и `NotSent`. Подтверждённый `NotSent` можно продолжить
с прежним ключом и неизменными параметрами/исходным временем, даже с новым алиасом.
Approval по-прежнему связан с точным видимым запросом и его алиасом. После TTL
`get_operation` читает сохранённую квитанцию при текущем праве на исходный scope.
Отдельные БД/восстановленные копии не разделяют admission; возраст на сервере
не доказывается. Контракт: [SPEC-mtproto-agent-callback.md](../SPEC-mtproto-agent-callback.md).

Офлайн-пример настоящего TL intake и двух вызовов с одной отправкой:
`cargo run --locked --offline -p tg-agent --features mtproto-test-kit --example mtproto_authorized_callback`.

`mtproto::updates::Feed` принимает целые партии M2: `ingest` сохраняет проекцию
и watermark в журнале, затем вызывает ACK. Хост удерживает один `SourceLease` и
`UpdateReceiver`; разрыв возвращается как `Ingested::Gap` для явного решения.
Для своего pipeline есть `store_batch` и `store_gap`, которые ACK не вызывают.
Проверка `checkpoint_acknowledged` означает подтверждение конкретной партии;
`false` допускает неопределённый исход, а `true` не исключает будущие разрывы.

Повтор последней партии не добавляет строки и не обновляет время callback,
включая перезапуск и истечение retention. Смена storage source требует отдельного
`accept_source_change` и записи discontinuity; перед этим хост завершает старый
Client. Общие лимиты feeds/строк/байтов учитывают Bot API и MTProto вместе.
`read` — доверенный доступ к хранилищу, без выдачи агентных разрешений.
Проекция содержит основные сообщения, удаления, read positions и callbacks;
неподдержанный TL constructor виден как unscoped marker. Graphs entities, access
hashes, file references и callback query ID в сериализованный вывод не входят.
Монотонное наблюдение callback локальное, теряется после рестарта и не доказывает
возраст запроса на сервере. Executor выдаёт callback references и принимает
host-only файлы. `mtproto-test-kit` — явная опция для офлайн-тестов,
не настройка инструмента агента.

`mtproto::updates::Reader::new(feed, policy, ReadOptions::default())` открывает
авторизованный путь `mtproto_read_updates`. `dry_run` и `approve` проверяют текущие
права и метаданные курсора без расхода квот и чтения тела. `read` списывает
долговечную квоту, рассматривает до 100 событий в пределах лимита байтов и выдаёт
только разрешённые основные peers. Для записей без peer нужны явные
`unscoped_updates`; sender/actor не открывает доступ. Смена grant перед выдачей
закрывает страницу. Отмена не возвращает уже списанную квоту и удерживает слот,
пока native-работа не завершится. По умолчанию — 16 read jobs и 1 MiB сохранённых
байтов на чтение. Вывод помечен `untrusted: true` и не содержит исполняемых callback
references. Контракт: [SPEC-mtproto-agent-read.md](../SPEC-mtproto-agent-read.md).

Отдельный пакет для постоянного состояния и инструментов агентов. Реализованы SQLite-журнал операций, хранение updates, checkpoint polling, независимые подтверждения потребителей, HTTP webhook и авторизованный исполнитель восьми действий A1. Реализованы общий JSON-интерфейс, самостоятельный CLI и MCP stdio через официальный Rust SDK; все используют один исполнитель. Хранилище не выдаёт права Telegram: principal/profile поступают от доверенного хоста, который обязан проверять текущие разрешения перед вызовом и выдачей сохранённого результата.

## Типы инструментов и политика A1

Опция executor открывает tg_agent::executor без рантаймов bot/webhook. A1 реализует Request::from_json, восемь фиксированных Action/ActionCall, PolicyStore и host-only Principal/Grant/Approval. Разрешения по умолчанию отсутствуют; текущая политика проверяет bot/profile/action/chat/file/expiry. Approval привязан к экземпляру политики, её ревизии и всему запросу с operation_id. Замена/отзыв grant аннулирует старое подтверждение. Telegram ID передаются каноническими десятичными строками, чтобы JSON-клиенты не теряли точность.

PolicyStore::dry_run выполняет только локальные проверки: нет сети, расхода квот и изменений журнала. Для callback и get_operation ему необходим контекст (ContextRequired). Executor::dry_run и Executor::approve разрешают зарегистрированный callback или неизменяемые метаданные файла/операции с проверкой владельца, не читая тело квитанции и не обновляя её состояние. Approval самого get_operation учитывается; повторно подтверждать исходную запись для чтения её статуса не нужно. DryRun проверяет разрешения и метаданные, но не обещает наличие активных байтов, доступную квоту, разрешение начать запрос или успех Telegram. Контракт: SPEC-executor.md в корне.

Executor::new(policy, options) и register_profile(feed, bot) объединяют текущие права, долговечные квоты и журнал. execute поддерживает get_me, get_chat, read_updates, get_operation, send_message, edit_message_text, answer_callback_query и send_document. Профиль неизменяем и связан с bot ID через UpdateFeed. Первый разрешённый удалённый вызов проверяет getMe; сохранённая квитанция читается после перезапуска без identity/send RPC. Политика повторно проверяется внутри транзакций квот и перед RPC/выдачей результата. Отзыв после отправки не отменяет действие: квитанция сохраняется, но выдача запрещается.

get_operation проверяет владельца и сохранённые исходные action/bot/chat/file/global scope по текущим правам. Отзыв исходного действия или ресурса закрывает старую квитанцию. Старые ручные J1-записи без scope не получают его из произвольного cached JSON и не выдаются исполнителем. Истечение срока тела успешной квитанции оставляет Succeeded с result_availability=expired, без повторной отправки. get_me/get_chat возвращают allowlist полей; get_chat проверяет ID ответа. Тела событий, invite links, неизвестные поля и описания ошибок в эти проекции не попадают.

read_updates читает только локальный feed: максимум 100 рассмотренных событий за вызов, не больше limit разрешённых событий и сырого byte budget ExecutorOptions::max_read_bytes (по умолчанию 1 MiB, максимум 64 MiB). Этот лимит относится к исходному JSON; DTO и экранирование ответа требуют дополнительной памяти. Потребление страницы перемещает payload без полного клонирования. Если первое событие не помещается, возвращается Capacity без пропуска. Cursor останавливается на последнем рассмотренном событии; лимит ответа не пропускает следующие строки. Проверка cursor в dry-run читает только позиционные метаданные и возвращает CursorExpired для истёкшей позиции. Чтение не запускает getUpdates, identity RPC или другой источник и не подтверждает consumer position.

Каждый ReadEvent содержит untrusted=true, канонические строковые update/chat/message/thread/callback ID и полный telegram_json как строку без потери точности вложенных ID. Rust-клиент получает типизированный Update через update(). Область события определяется по первичному известному событию, отдельно от ordering hint и пересланных/nested chat ID. Неизвестные корневые поля, guest/widget/inline/account требуют отдельного unscoped grant; он дополнительно учитывает узнаваемый обычный chat и не обходит его запрет. Неоднозначный message+inline callback не выдаёт action metadata. Cursor не является разрешением: права повторно проверяются при каждом чтении. Содержимое Telegram не меняет grant или описание инструмента.

Callback появляется в action metadata только после регистрации разрешённым read_updates или host-only observe_callback(principal, profile, stored_update). Нужен снимок того же экземпляра журнала/профиля и текущее право чтения либо ответа в фактическом chat scope; inline требует unscoped grant. Реестр ограничен 1024 ссылками, TTL — 10 секунд от первоначального приёма (настраивается до 60 секунд). Повторное чтение не продлевает срок. Переполнение/истечение убирает action metadata, сохраняя разрешённое содержимое события. По умолчанию JournalMonotonic использует приватную монотонную отметку свежего commit: после перезапуска или чтения через другой экземпляр журнала старый callback недоступен для новой отправки. TrustedWallClock явно разрешается хостом для таких случаев и требует корректных UTC-часов. Это не аргумент инструмента. get_operation позволяет восстановить квитанцию независимо от живой ссылки. Долговечный claim callback общий для всех principals в профиле: после допуска нельзя обойти Unknown новым operation_id; подтверждённый NotSent повторяется только с прежним ключом. Успех сохраняет только acknowledged=true. edit_message_text принимает обычный chat/message target и проверяет оба ID ответа; boolean-only ответ считается Unknown.

register_file(principal, profile, filename, bytes, expires_at_ms) принимает owned Vec от доверенного хоста и возвращает RegisteredFile с неприменяемым повторно handle. Затем хост отдельно разрешает handle в Grant::files. Имя — до 255 байтов, без путей/control chars; в аргументах send_document нет URL, пути или Telegram file_id. SQLite v5 сохраняет владельца, профиль, SHA-256, размер, имя и expiry без содержимого файла. restore_file после перезапуска требует точного хеша/размера и владельца; discard_file/prune_files освобождают ссылки реестра. Старые метаданные не удаляются/не перезаписываются: JournalOptions::max_file_handles ограничивает их 100000 записями.

ExecutorOptions::file_limits(FileLimits) по умолчанию ограничивает файл 50 MiB, снимки 100 MiB, число активных/ожидающих снимков 1024 и одновременные upload 50 MiB. Учитывается полная Vec::capacity() до любого ожидания БД/хеширования. Байты и permits переходят в shared owner multipart без полного копирования; удаление снимка/отмена ожидания не освобождает память, которой ещё владеет transport. Новый upload требует живого неизменяемого снимка; expiry проверяется перед identity, в begin и перед SDK, ограничивая deadline. Expiry файла и grant полагаются на корректные UTC-часы хоста. Сохранённая квитанция остаётся доступной по текущим правам после истечения/удаления байтов. Подробный контракт: SPEC-agent-files.md.

По умолчанию ExecutorOptions ограничивает 16 одновременных вызовов, 30 секунд на RPC и 5 секунд на запись квитанции; суммарный бюджет должен укладываться в lease журнала. Deadline привязан к фактическому сроку lease и оставшемуся сроку callback/файла; ожидание политики не продлевает его. Есть одна попытка SDK без неявных повторов. Тайм-аут, неверный target в ответе и неопределённая запись квитанции не превращаются в NotSent. Отмена после begin оставляет InFlight, затем Unknown; повтор с тем же ключом читает состояние. Любая ошибка относится к тому же operation_id: даже NotSent не разрешает автоматически заменять ключ или игнорировать имеющуюся квитанцию.

Budget имеет постоянный ID и фиксированное окно (60 секунд): по умолчанию 100 разрешённых вызовов, 20 допусков записи, 50 MiB upload и 1000 условных единиц стоимости. Стоимость — оценка хоста, стандартно 1 на действие; платные параметры Telegram не открыты. Кешированный write расходует вызов, но не новый write/upload/cost. Каждая новая попытка после подтверждённого NotSent расходует новый допуск. SQLite v4 сохраняет счётчики и исходный scope; смена ревизии grant/перезапуск не сбрасывает расход. Срок окна у существующего budget ID нельзя менять; текущие caps применяются к сохранённому расходу. Откат часов не открывает старое окно. JournalOptions::max_budget_ledgers по умолчанию 4096: заполнение отклоняет новый ID без вытеснения. Каждый допуск upload расходует полный неизменяемый размер файла, включая повтор того же ключа после подтверждённого NotSent.

```powershell
cargo test --locked -p tg-agent --features executor --test executor_policy
cargo test --locked -p tg-agent --features executor --test executor_send
cargo test --locked -p tg-agent --features executor --test executor_read
cargo test --locked -p tg-agent --features executor --test executor_callback --test executor_edit
cargo test --locked -p tg-agent --features executor --test executor_files --test migrations
cargo run --locked --offline -p tg-agent --features executor --example authorized_send
cargo run --locked --offline -p tg-agent --features executor --example authorized_read
cargo run --locked --offline -p tg-agent --features executor --example authorized_callback
cargo run --locked --offline -p tg-agent --features executor --example authorized_document
```

## Общий JSON-интерфейс и CLI (A2)

tools::ToolSession создаётся доверенным Rust-хостом из Executor и Principal. execute_for дополнительно связывает выбранное имя инструмента с Request.call.tool до допуска; входной JSON не получает прав задавать principal, grant или approval. Host-only approve хранит точное подтверждение запроса; clear_approvals удаляет сохранённые подтверждения для последующих вызовов. Для отзыва capability, уже переданного выполняющемуся вызову, нужно изменить/отозвать grant в PolicyStore; допущенный удалённый эффект нельзя отменить. Текущие права и ревизия политики проверяются исполнителем и при чтении кеша.

При опции `mtproto` хост явно подключает второй исполнитель:

```rust,no_run
use tg_agent::{executor::{Executor, Principal}, tools::ToolSession};
fn session(bot: Executor, mt: tg_agent::mtproto::Executor, principal: Principal) -> ToolSession {
    ToolSession::new(bot, principal).with_mtproto(mt)
}
```

`ToolRequest::from_json` принимает строго один из существующих запросов без
смешения типов. `execute_named` проверяет отдельное выбранное имя до допуска;
`approve_mtproto` сохраняет точное host-подтверждение в общем ограниченном
хранилище. Ошибки MT-записи направляют к `mtproto_get_operation` с исходным
ключом; обрыв доставки оставляет `unknown`. Хост сначала закрывает MT Executor,
затем Client, затем Journal; `ToolSession` не владеет их завершением.

`session.catalog()` содержит 8 Bot инструментов, либо 16 при явном подключении
MT. Команды `tools --mtproto` и `schema [request|response|catalog] --mtproto`
открывают расширенный каталог/набор схем версии 2 без конфигурации или RPC.
Отдельные запросы и ответы сохраняют `schema_version=1`. Прежние схемы `v1`
не изменены; `python tools/agent_schema.py check --mtproto` проверяет `schemas/v2`.
Самостоятельный `Host::load` принимает прежнюю Bot-конфигурацию v1 и смешанную
v2 с явной секцией `mtproto`. Он открывает подготовленные Session без RPC,
проверяет точный аккаунт, восстанавливает файлы/наблюдения и выдаёт текущие
подтверждения. `Host::close(&self)` допускает повторное ожидание и закрывает
MT Executor → все Clients → Journal. [Настройка MTProto](../docs/AGENT-MTPROTO.md).

Ответ имеет schema_version=1, ok и result/dry_run/error. ok=true означает получение результата исполнителя: для записи нужно проверить operation.state и result_availability. Ошибки содержат статический code/message и automatic_retry_allowed=false; ошибки проверенной записи сохраняют operation_id и next_action=get_operation. Сбой вывода после действия также сохраняет ключ и unknown; Debug/Display не показывают payload или идентификаторы.

Опция cli включает встроенные cli::run_json/serve_jsonl и бинарный tg-agent с tools/schema/run/serve. run --config ABSOLUTE_PATH принимает один JSON-документ, serve --config ABSOLUTE_PATH — NDJSON; --dry-run проверяет запрос без отправки. Кадр ограничен 65536 байтами, output по умолчанию — 8 MiB. Слишком большой кадр закрывает поток, обычный некорректный кадр получает ошибку без выполнения. Все восемь действий используют общий исполнитель. [Полное руководство CLI](../docs/AGENT-CLI.md).

Опция host отдельно открывает Host::load для встраиваемого приложения/MCP. Конфигурация оператора задаёт principal, абсолютный путь журнала, профили, grants и имена переменных окружения токенов. Неизменяемые файлы восстанавливаются только по заранее выданному handle и совпадающим байтам. Подтверждения содержат точный Request и expiry; callback_observations связывает точный cursor события с текущими scope/TTL перед подтверждением. Межпроцессные callback требуют явного trusted_wall_clock; модель не может задавать эти host-параметры через tool JSON. Загрузка выполняет статические проверки до открытия БД, не отправляет RPC и закрывает журнал при последующем сбое. EOF завершает работу с явным close.

```powershell
cargo run --locked --offline -p tg-agent --features cli --bin tg-agent -- tools
cargo run --locked --offline -p tg-agent --features cli --bin tg-agent -- schema request
cargo run --locked --offline -p tg-agent --features cli --bin tg-agent -- run --config C:/protected/host.json --dry-run
cargo run --locked --offline -p tg-agent --features cli --example agent_json
python tools/agent_schema.py check
```

Опция `mcp` включает команду `tg-agent mcp --config ABSOLUTE_PATH [--dry-run]`
и встраиваемый `mcp::McpServer`. Восемь инструментов, схемы и аннотации берутся
из общего каталога; аргументы — полный Request версии 1. SDK обрабатывает
JSON-RPC, raw guard отклоняет дубли ключей и кадры больше 128 KiB. Отмена или
потеря ответа не повторяет Telegram; восстановление идёт по исходному ключу.
[Руководство MCP, ограничения и офлайн-оценка](../docs/AGENT-MCP.md).

Каталог содержит все восемь инструментов, JSON Schema draft 2020-12, эффекты/правила повторов и примеры. Схемы schemas/v1 воспроизводятся командой python tools/agent_schema.py generate через тот же бинарник. JSON Schema проверяет структуру и точные границы строковых i64 ID; байтовые лимиты, дубликаты JSON-полей, доступность references и grants окончательно проверяются в runtime. Discovery не читает токены/БД и не выполняет RPC. Пример agent_json использует синтетический транспорт и выводит два JSON-ответа с одной отправкой.

## Использование журнала

```powershell
cargo run --locked --offline -p tg-agent --example journal
```

Он дважды вызывает одну операцию с постоянным ключом и проверяет, что отправка через тестовый транспорт выполнена один раз. Сеть и настоящие токены не используются. Полный код: examples/journal.rs.

Рабочий порядок: создать OperationRequest с profile, principal, operation_id, action и каноническими JSON-параметрами; вызвать prepare; только для Preparation::Ready вызвать begin, дождаться его успешного commit и начать действие. Ответ превратить в минимальный Completion::succeeded или Completion::from_error и сохранить через complete. Preparation::Existing возвращает состояние/результат без нового действия.

Ключ состоит из profile и operation_id. Владелец, действие и хеш параметров закрепляются первым резервированием. Другой владелец не получает запись; другой payload/action даёт Conflict. Для проверки файлов исполнитель должен включать неизменяемый handle и хеш содержимого в параметры — сам журнал файлов не открывает. Известный NotSent освобождает запись в prepared с новой версией. Rejected, Unknown и Succeeded не дают новый исполняемый lease. После исправленного явного отказа новое действие требует нового operation_id; неопределённый исход нельзя обходить новым ключом без проверки.

recover_expired(limit) переводит только истёкшие in_flight в unknown; активные попытки других процессов сохраняются. get/prepare также проверяют срок конкретной доступной записи. Истёкший prepared можно занять снова до общего срока жизни. Версия и случайный ID экземпляра защищают begin/complete от старого или чужого lease. Срок проверяется после получения блокировки SQLite. Истечение lease после in_flight никогда не разрешает повторную отправку. Если неизменённый владелец получил позднее подтверждение до восстановления записи, он может сохранить его; после fencing старой версии это невозможно.

По умолчанию: 100 000 ключей, 16 ожидающих/выполняющихся задач БД на экземпляр, lease 60 секунд, срок операции/результата 7 суток от первого резервирования, JSON до 64 KiB, глубина до 64 уровней и 8192 узлов. Хост может изменить лимиты через JournalOptions. Сетевой deadline с запасом на запись результата должен помещаться в journal.lease_duration(). Нагрузка сверх лимита БД получает Capacity. Отмена async-вызова не останавливает уже начавшуюся транзакцию; её permit остаётся занятым до завершения. После отмены/сбоя нужно читать тот же ключ.

prune_expired(limit) удаляет доступное содержимое результата и помечает срок истёкшим, сохраняя ключ, владельца, хеш и состояние. Автоматического удаления ключей нет: заполнение приводит к Capacity, а не к новой отправке старого действия. Ротация namespace/архива оператором явно завершает прежнюю гарантию. Результат может иметь availability stored, disabled, too_large или expired; отсутствие тела при succeeded не разрешает повтор. retain_results(false) отключает запись тел новых результатов. Журнал не читает токены и не сохраняет исходные параметры, файлы или тексты удалённых ошибок; содержимое явно переданного результата определяет хост; retry_after и migrate_to_chat_id сохраняются как советы, без повторов или смены адресата.

Нужен абсолютный путь на локальном диске в защищённом каталоге. Родительский каталог создаёт и защищает хост. Новый Unix-файл имеет 0600; на Windows права наследуются от каталога. SQLite работает с WAL и synchronous=FULL; неподходящая версия или структура отклоняется до смены journal_mode. Результаты, явно переданные хостом, могут содержать приватные данные в открытом виде. TTL удаляет доступ через API, но не обещает немедленное физическое стирание старых WAL-страниц, резервных копий или данных носителя. Защита и сроки хранения этих копий относятся к оператору.

Соединением владеет выделенный поток: даже последний Drop не закрывает SQLite на async-потоке. close().await прекращает приём задач для всех клонов, дожидается уже принятых задач и закрытия БД. Штатное закрытие освобождает только не начатые резервы этого экземпляра, с новой версией; in_flight не становится повторяемым. Хост сначала останавливает/дожидается своих внешних действий и затем закрывает журнал. После close новые вызовы получают journal_closed; повторный close безопасен. max_pending имеет верхний предел 1024.

Журнал подавляет дубли внутри исполнителя и сохраняет неопределённость при падении между действием и записью ответа. Он не создаёт транзакцию с Telegram и не обещает exactly-once внешних эффектов.

## Входящие события

UpdateFeed::open связывает профиль с проверенным хостом bot_id и UpdateOptions. Другой bot_id для этого профиля, второй alias того же бота или другая политика существующего feed отклоняются. claim_source(SourceMode::Webhook) выдаёт один versioned lease; renew_source продлевает только ещё действующее владение, release_source освобождает его. Работают те же гарантии между процессами, что у журнала операций; close освобождает только source-lease своего экземпляра.

append_webhook принимает уже проверенный Update и возвращает receipt после commit. Этот API не аутентифицирует HTTP: адаптер обязан сделать это до вызова. Одинаковый update_id с тем же содержимым отсекается; конфликтующее содержимое не заменяет старое. Локальная последовательность независима от Telegram ID, поэтому запоздавшие webhook-события не теряются из-за сортировки. Неизвестные поля сохраняются. chat_id используется для порядка и не является разрешением читать данные.

read(cursor, limit) возвращает UpdatePage с events, next_cursor и has_more. Cursor — ограниченная строка с версией и привязкой к профилю, без роли access token; он сохраняет точность больших позиций. Чтение не подтверждает обработку. Чужой/future cursor отклоняется, устаревший возвращает updates_cursor_expired. Новый read(None, ...) начинает текущее окно хранения. Debug скрывает содержимое событий и cursor.

Лимиты JournalOptions общие для базы: max_update_feeds (64), max_update_entries (100 000), max_update_bytes (64 MiB). UpdateOptions задаёт retention (7 суток), max_event_bytes и max_batch_bytes (по 1 MiB). read ограничен 1..100 событиями и byte budget. Новое событие при заполнении отклоняется, не вытесняя действующие данные. prune_updates(1..1000) удаляет истёкшие входящие записи, возвращает их квоту и продвигает retention floor; локальные позиции не переиспользуются. Это отличается от бессрочно удерживаемых ключей операций: дедупликация входящих событий гарантируется только в выбранном окне хранения.

append_poll принимает lease, полученный poll_checkpoint и целый упорядоченный пакет (до 100). События, квоты и следующий offset сохраняются одной транзакцией. Устаревший checkpoint отклоняется; после отмены нужно перечитать его из журнала. Пустой ответ не продлевает свежесть offset: после 24 часов без непустого пакета он не отправляется. Переключение polling/webhook инвалидирует старый offset.

claim_consumer(id) создаёт независимую позицию обработки и versioned lease. read_consumer читает пакет без продвижения позиции; ConsumerBatch::into_parts выдаёт события и непрозрачное подтверждение. acknowledge_consumer продвигает позицию только для того же владельца, поколения и начальной позиции. Подтверждение остаётся действительным при renew_consumer, но не при release и новом claim. Потерянное владение возвращает LeaseLost. После перезапуска неподтверждённые события читаются снова: внешние эффекты обработчика требуют стабильных operation_id. skip_expired_consumer явно пропускает только уже истёкший префикс после CursorExpired. max_update_consumers (256) ограничивает число ID во всей базе; освобождённые ID сохраняют позиции и занимают квоту.

Миграция v1/v2/v3→v4 выполняется атомарно и сохраняет операции, хеши, результаты, обновления, polling-позиции и активные версии. Для офлайн-проверки хранилища:

```powershell
cargo run --locked --offline -p tg-agent --example update_store
```

Пример не поднимает HTTP-сервер и не обращается к Telegram. Прогресс приёма и обработки разделён.

## Управляемый рантайм

```toml
tg-agent = { path = "../gramrelay/tg-agent", features = ["bot"] }
```

Опция bot открывает tg_agent::runtime::{PollOptions, ConsumerOptions, RuntimeError} и методы UpdateFeed::run_polling_until / run_consumer_until. По умолчанию tg-agent включает только хранилище.

run_polling_until(&bot, options, stop) проверяет getMe и bot_id feed, отказывает при действующем webhook, занимает SDK guard и source lease, затем сохраняет каждый полный пакет до следующего getUpdates. По умолчанию: 100 событий, long poll 20 секунд, общий request_budget 30 секунд, empty_poll_delay 250 мс. request_budget не превышает половины lease и превышает long poll; timeout клиента должен его вмещать. Ошибка останавливает цикл без неявного повтора. Повторный запуск читает сохранённый offset. После заполнения хранилища хост освобождает истёкшие записи через prune_updates и решает, когда перезапускать источник.

run_consumer_until(&bot, consumer_id, router, options, stop) независимо проверяет личность и привязывает команды к username, занимает consumer lease и повторяет неподтверждённую позицию. DispatchOptions ограничивает параллелизм (16 групп чатов) и каждый обработчик (30 секунд); события одного чата идут последовательно, неизвестная область сериализует пачку. По умолчанию batch_limit 100, idle_delay 250 мс, shutdown_timeout 30 секунд. Обработка обновляет lease; ожидание SQLite также ограничено известным сроком владения. Ошибка, panic, timeout или потеря lease сохраняют прежнюю позицию.

Для штатной остановки вызовите stop.cancel() и дождитесь результата рантайма. Потребитель завершает принятую пачку в shutdown_timeout; успешная пачка подтверждается, незавершённая повторяется после восстановления. Подтверждение и освобождение владения ждут принятые задачи БД, включая задачи с уже отменённым async-ожиданием; обычные операции сохраняют fail-fast Capacity. Очистка БД ожидается отдельно от deadline обработчиков. close будит ожидающих и освобождает аренды своего экземпляра. Drop всего future не может выполнять async-очистку: аренда остаётся до истечения срока. Обработчики должны уступать управление Tokio; независимо запущенные ими задачи и уже начавшиеся внешние эффекты контролирует хост.

Полный офлайн-пример устойчивого приёма и подтверждения двух событий:

```powershell
cargo run --locked --offline -p tg-agent --features bot --example durable_bot
```

Тесты принудительно завершают настоящие дочерние процессы после получения, commit приёма, синтетического эффекта обработчика и подтверждения подписчика. Дополнительно проверяются конкурирующие процессы, восстановление offset, заполнение, heartbeat под блокировкой SQLite и отмена при max_pending(1). Разрешения на чтение и действия добавит A1.

## Устойчивый webhook

```toml
[dependencies]
tg = { path = "../gramrelay" }
tg-agent = { path = "../gramrelay/tg-agent", features = ["webhook"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net"] }
```

Опция webhook включает bot и общий HTTP-адаптер tg. UpdateFeed::webhook(&bot, secret) создаёт ленивый DurableWebhook; options(WebhookOptions) задаёт те же лимиты HTTP, что у обычного бота. serve_until(listener, stop) проверяет getMe/feed bot_id, занимает SDK guard и SourceMode::Webhook, затем отвечает 200 после append_webhook commit. Повтор с тем же fingerprint получает 200 без новой строки, конфликт — 409, заполнение — 503. События остаются в журнале после перезапуска; обработка идёт через независимый consumer.

```rust,no_run
use tg::{Bot, CancellationToken};
use tg::webhook::WebhookSecret;
use tg_agent::{updates::UpdateFeed, runtime::Result};

async fn serve(feed: &UpdateFeed, bot: &Bot,
    listener: tokio::net::TcpListener, secret: WebhookSecret,
    stop: CancellationToken) -> Result<()> {
    feed.webhook(bot, secret).serve_until(listener, stop).await
}
```

По умолчанию общий request_timeout 30 секунд; он должен занимать не больше половины journal lease (60 секунд). Heartbeat работает и в простое, ждёт место в ограниченной очереди БД и ограничивает ожидание продления известным сроком владения. Потеря lease закрывает приём; runtime не пытается заново занять источник. Source lease освобождается после HTTP drain и всех принятых native jobs, затем освобождается SDK guard. Такая очистка БД ожидается отдельно от HTTP shutdown deadline. Если оборвать весь future вместо stop.cancel() + await, уже принятая запись может закончиться, а DB lease остаётся до истечения срока. Удалённый setWebhook/deleteWebhook выполняет хост явно; HTTP/1 listener по умолчанию loopback, TLS обслуживает хост или доверенный reverse proxy.

Опции tracing и metrics перенаправляются в SDK. Диагностика webhook содержит только фиксированные статусы/причины, длительность и число активных POST-pipeline; ID, пути, секреты и тела в неё не включаются. Subscriber/recorder устанавливает приложение.

Полный локальный пример с SQLite и отдельным consumer:

```powershell
cargo run --locked --offline -p tg-agent --features webhook --example durable_webhook
```
