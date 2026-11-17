# Компилятор и платформы

Три библиотеки объявляют `rust-version = "1.89"`. Первая реальная проверка
Rust 1.89.0 / Windows x86_64 MSVC прошла 681 тест со всеми возможностями.
Полная таблица ниже обновляется только после фактических прогонов.

| Платформа | Rust 1.89.0 | Текущий stable |
|---|---|---|
| Windows x86_64 MSVC | 28 стадий прошли | 1.100.0: 28 стадий прошли |
| Linux x86_64 | Не проверено | Не проверено |
| macOS ARM64 | Не проверено | Не проверено |

Подтверждённый MSRV-прогон: 395 default-тестов, 681 all-feature тест,
16 doctests и 25 production feature-конфигураций. Его завершённый кэш
удалён штатной командой Cargo: 8,4 ГиБ; небольшие журналы сохранены.
Источник: `target/compat-1.89.0-evidence/run-7ianukp5/summary.json`.
Те же количества тестов/конфигураций прошли на 1.100.0:
`target/compat-1.100.0-evidence/run-k59txsmg/summary.json`. Отдельный Clippy
новой stable выявил переименование AtomicU64::fetch_update. Новый try_update
экспериментально не компилируется на 1.89. Существующий checked update сохранён
без изменения поведения; точечная пометка deprecation в advance_epoch объясняет
поддержку MSRV. Общее `-D warnings` сохраняется. Итоговый Clippy и повторная
реальная MSRV-компиляция этой пометки проходят; существующий runtime-тест
исчерпания epoch повторно прошёл на 1.100.0.

Установленный псевдоним stable может отставать. `rustup check` на 17.11.2026
показал 1.100.0; локальный default 1.98.0 сохранён. Для повторения:

```powershell
rustup toolchain install 1.89.0 --profile minimal --no-self-update
python tools/check_compat.py --toolchain 1.89.0
rustup toolchain install 1.100.0 --profile minimal --component rustfmt,clippy --no-self-update
python tools/check_compat.py --toolchain 1.100.0
```

Скрипт выполняет default/all-feature тесты workspace, all-feature doctests и
отдельные production `--lib` проверки без default features: минимальную,
каждый feature, все вместе, а также host/cli/mcp вместе с mtproto. Список
берётся из `cargo metadata --no-deps`, включая неявные features optional
dependencies. Это заданная матрица сочетаний, а не полный перебор всех подмножеств.

Каждый запуск печатает свой каталог
`target/compat-<toolchain>-evidence/run-<id>`. Его `summary.json` содержит точный
rustc, платформу, commit/dirty, хеш Cargo.lock, команды, коды завершения и ссылки
на журналы. `passed` относится только к указанному suite: режимы `--suite tests`
и `--suite features` проверяют соответствующую часть. Прерывание сохраняет
начатую команду с неизвестным кодом завершения; новый запуск не смешивает
доказательства с предыдущим или параллельным. Ошибки preflight сохраняют
доступный контекст и диагностику Cargo.

Кэш отдельный: `target/compat-<toolchain>`. После завершения процесса:

```powershell
cargo clean --target-dir target/compat-1.89.0 --profile dev
```

Не очищайте живую сборку. Скрипт проверяет свободные 20 ГиБ и лимит `target`
20 ГиБ перед стадиями и после них; компактный профиль Cargo сохраняется.
Workflow `.github/workflows/compatibility.yml` готовит шесть native-ячеек,
проверку схем/SDK, Clippy/fmt, source-кандидат и отдельный аудит. Его наличие
не доказывает успешную работу на Linux/macOS: workflow ещё не запускался.
Он использует read-only permissions, точные SHA официальных actions и сохраняет
небольшие доказательства после очистки сборок. Standard hosted runners гарантируют
только 14 ГБ SSD: preflight обязан отказать, если фактическое свободное место
меньше 20 ГиБ; тогда требуется runner с подходящим диском. Уменьшения локального
лимита и удаления предустановленного ПО workflow не делает.
Перед relocated-проверкой завершённый native-кэш очищается; общий размер
проверяется и после Clippy/consumer. Отдельный audit job тоже имеет preflight,
проверку размера после сборки и очистку своего завершённого release-кэша.

MSRV относится к проверенному исходному кандидату и его Cargo.lock. Новый
compatible upstream-релиз может потребовать более новый Rust; Cargo учитывает
rust-version при разрешении, но независимый внешний lock также нужно проверять.
Повышение MSRV требует явной записи в release notes и повторения матрицы.
Общий gate Q3 остаётся открытым до всех native-ячеек и реестровой упаковки.
