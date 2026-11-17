# MTProto в CLI и MCP

Соберите `tg-agent` с `--features mcp,mtproto`. `Host::load` принимает
конфигурацию v2 с подготовленными MTProto-сессиями; прежние Bot-профили могут
находиться рядом в массиве `profiles`. Principal, текущая политика и журнал
общие. MCP объявляет 16 инструментов. Отдельные запросы/ответы остаются версии 1.

```powershell
cargo build --locked -p tg-agent --features mcp,mtproto --bin tg-agent
./target/debug/tg-agent.exe tools --mtproto
./target/debug/tg-agent.exe schema request --mtproto
./target/debug/tg-agent.exe mcp --config C:/protected/host.json
```

## Подготовка сессии

Доверенный Rust-хост предварительно создаёт Session, выполняет вход и проверяет
точную личность через [M2 API](../tg-mtproto/README.md), затем закрывает Client.
Сохраните тот же `session_binding`, `api_id` и ключ. Ключ — 32 случайных байта,
переданные процессу через указанную переменную окружения как 64 hex-цифры.
Конфигурацию, окружение, каталог сессии и журнал защищает оператор средствами ОС.

Пример только для чтения с вымышленными ID и абсолютными путями:

```json
{
  "schema_version": 2,
  "principal": "local-agent",
  "journal_path": "C:/protected/agent.sqlite",
  "profiles": [],
  "mtproto": {
    "profiles": [{
      "name": "personal",
      "account_kind": "user",
      "account_id": "123456789",
      "session_path": "C:/protected/personal.sqlite",
      "session_binding": "personal",
      "api_id": 12345,
      "session_key_env": "TG_PERSONAL_SESSION_KEY",
      "grant": {
        "actions": ["mtproto_get_me", "mtproto_get_peer", "mtproto_read_updates", "mtproto_get_operation"],
        "peers": [{"kind": "user", "id": "987654321"}],
        "budget": {"id": "personal-daily"}
      }
    }]
  }
}
```

`account_kind` равен `user` или `bot`; ID — каноническая положительная строка.
Загрузка сверяет ключ, binding и сохранённую личность без RPC; каждое удалённое
действие отдельно проверяет live self. `Session::open_existing` не создаёт файл
или схему. Отсутствующие, пустые, чужие и quarantined Session отклоняются.
Имена профилей общие; дубли точного аккаунта и файловые алиасы, включая hard
links, отклоняются. Предыдущий владелец Session должен завершить работу.

MT-профиль поддерживает `request_timeout_ms` 1..30000 (default 30000),
`max_pending` 1..1024 (16) и прежний `update_options`. Grant принимает
`actions`, `peers` либо `all_peers`, `files`, `unscoped_updates`, `expires_at_ms`,
`require_approval`, `costs` и общий `budget`. Пустые права запрещают доступ;
дубли списков и противоречивые области отклоняются. `get_peer` проверяет кеш;
подготовку собеседников выполняет доверенный Rust-хост.

Общие пределы: 1 MiB конфигурации, 64 профиля, 256 подтверждений, 1024
восстановления файлов и 1024 наблюдения callback для обоих протоколов вместе.
Файлы ограничены двумя резидентными пулами по 100 MiB, по одному на протокол;
при загрузке читается один дополнительный ограниченный файл.

## Записи и восстановление

Оператор добавляет действия записи и ресурсы в grant. Для `require_approval`
секция `mtproto.approvals` содержит точный `{request, expires_at_ms}`. Expiry
задаётся хостом в миллисекундах UTC. Другой текст или operation_id требует нового
подтверждения. `mtproto.file_restorations` содержит `{profile,file_handle,path}`
для ранее зарегистрированных handles: owner/профиль/SHA-256/размер должны совпасть,
исходный срок не продлевается. Инструмент получает только handle.

Для callback бота `mtproto.callback_observations` содержит
`{profile,cursor,reference}`. Хост связывает `mtcb_` + 32 строчные hex-цифры с
точным событием через `observe_callback_as`, чтобы alias был известен точному
approval. Среди загруженных наблюдений aliases уникальны; между запусками хост
повторяет alias только для исходного события. Peer, provenance и оригинальный
TTL проверяются заново; alias не обновляет срок. Межпроцессные наблюдения требуют
явного корневого `"callback_clock_policy":"trusted_wall_clock"`. Default
`journal_monotonic` отказывает без исходного монотонного времени. Истёкшие
наблюдения останавливают загрузку; уберите их из host-конфигурации для последующих
чтений сохранённой квитанции.

`ok:true` означает получение результата. Проверьте `operation.state` и
`result_availability`. Потерянный ответ и `unknown` восстанавливаются через
`mtproto_get_operation` с исходным ключом без автоматического повтора. Новый
SDK-запрос допустим только для подтверждённого NotSent с прежним ключом.

`read_updates` читает локальный журнал. Приём/ACK удалённых партий организует
Rust-хост через M2 receiver/Feed; загрузка CLI не запускает polling. Текст событий,
callback data и actor IDs остаются недоверенными данными.

После завершения вызовов `Host::close(&self)` ждёт MT Executor, все Clients и
native cleanup, затем закрывает Journal. Ожидание можно повторить после отмены;
ошибка одного ресурса не пропускает остальные. Drop/аварийный выход не обещают
joined shutdown. Частичный сбой загрузки закрывает принятые ресурсы; локальные
метаданные профилей могут сохраниться, удалённых действий при загрузке нет.

Разработческий `tg-agent-mtproto-fixture` доступен с `cli,mtproto-test-kit` и
предназначен для синтетических сессий/журналов. Обычный `tg-agent` всегда выбирает
`Client::new`; конфигурация не переключает SDK на фикстуру.
