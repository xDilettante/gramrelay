# Компилятор и платформы

Три библиотеки объявляют `rust-version = "1.89"`. Первая реальная проверка
Rust 1.89.0 / Windows x86_64 MSVC прошла 681 тест со всеми возможностями.
Полная таблица ниже обновляется только после фактических прогонов.

| Платформа | Rust 1.89.0 | Текущий stable |
|---|---|---|
| Windows x86_64 MSVC | 28 стадий прошли | 1.100.0: 28 стадий прошли |
| Linux x86_64 GNU (Ubuntu / WSL2) | 28 стадий прошли | 1.100.0: 28 стадий, fmt/Clippy прошли |
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

Linux/MSRV повторил те же 395/681/16 результатов и 25 production-конфигураций
из закрытого source-кандидата ff7bfca20752. После сборки все 649 исходных файлов
сверены с архивным манифестом. Журналы и идентификатор архива сохранены в
`target/compat-linux-1.89.0-evidence/run-megtrgr6`; завершённый кэш 8,1 ГиБ очищен.
Обнаруженные Linux-зависимости тестовой обвязки исправлены; production-лимит
20 ГиБ сохранён и имеет отдельный тест отказа до запуска команды.

Linux/1.100.0 повторил 395/681/16 и 25 production-конфигураций из кандидата
c5299cc83aa1; все 649 исходных файлов также сверены после выполнения.
`target/compat-linux-1.100.0-evidence/run-4vqk9kaq` содержит результаты и
идентификатор архива. Отдельные fmt/Clippy прошли с `-D warnings`; завершённый
кэш 8,7 ГиБ очищен. Эти результаты относятся к native Linux x86_64 GNU внутри
WSL2 и не подтверждают другие Linux ABI/архитектуры.

Архив 767a65ee630c с 656 проверенными файлами прошёл 43 Python-теста,
schema check и пять SDK-проверок в Linux. Его перенесённое внешнее приложение
успешно выполнило Bot message/callback/upload/Unknown, MCP discovery и MT
real-TL/joined close. Все восемь защищённых узлов графа взяты из распакованного
дерева; независимый lock из 278 пакетов прошёл аудит, как и основной из 337.
Журналы/идентичность сохранены в `target/compat-linux-source-consumer-evidence`,
`target/compat-linux-source-consumer.log` и `target/compat-linux-*-audit.log`.
Завершённый consumer-кэш 1,2 ГиБ очищен. Bundled SDK-входы устраняют зависимость
provenance-проверок от старого кэша; обычные registry-зависимости требуют
предварительно заполненного Cargo cache, затем consumer работает offline.

Кандидат `94269339316a` повторно проверен из одного и того же ZIP на Windows
x86_64 MSVC и Linux x86_64 GNU в WSL2. Обе версии, 1.89.0 и 1.100.0, прошли
по 28 стадий на Linux. На Windows 1.89.0 прошли все 28 стадий, а на 1.100.0
отдельно повторены 25 production-конфигураций в дополнение к уже пройденным
трём наборам тестов того же кандидата. Во всех прогонах workspace получились
412 default-тестов, 698 all-feature тестов и 16 doctests.

На Linux также прошли fmt, строгий Clippy, 56 Python-тестов, проверка схемы
и исходников пяти SDK, запуск внешнего приложения из распакованного дерева и
аудит его lockfile с 278 пакетами. После проверок байты 750 исходных файлов
совпали с архивом на обеих системах. На Linux сверили и режимы файлов.
Доказательства сохранены в `target/q3-win-9426933-evidence/source-after.json`
и `target/q3-linux-9426933-evidence/run-ai6te4ib/summary.json`. В Windows-отчёте
источник подтверждён сверкой с архивом. Старый скрипт брал Git-метаданные
внешнего репозитория при вложенной распаковке. Исправление покрыто регрессией
на Windows и Linux.

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
на журналы. У распакованного ZIP без собственного Git поля commit/dirty пусты.
Идентификатор такого прогона подтверждают SHA-256 архива и сверка исходных файлов.
`passed` относится только к указанному suite: режимы `--suite tests`
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
проверку схем/SDK, Clippy/fmt, source-кандидат и отдельный аудит. Сам workflow
ещё не запускался; локальные Linux-результаты выше не подтверждают macOS/CI.
Он использует read-only permissions, точные SHA официальных actions и сохраняет
небольшие доказательства после очистки сборок. Standard hosted runners гарантируют
только 14 ГБ SSD: preflight обязан отказать, если фактическое свободное место
меньше 20 ГиБ; тогда требуется runner с подходящим диском. Уменьшения локального
лимита и удаления предустановленного ПО workflow не делает.
Перед relocated-проверкой завершённый native-кэш очищается; общий размер
проверяется и после Clippy/consumer. Отдельный audit job тоже имеет preflight,
проверку размера после сборки и очистку своего завершённого release-кэша.
Для cold cache перед проверками выполняется `cargo fetch --locked` без
`--target`: независимый metadata-граф включает также не-host платформы.
Audit job проверяет основной lock и по одному независимому consumer-lock
из каждой из шести успешных native-ячеек текущего workflow run. Неполный или
дублированный набор не принимается. Локальные actionlint и проверка Python-шага
на шести входах/пропуске/дубликате/ошибке аудитора прошли; это не запуск CI.

## Локальная приёмка закрытого ZIP на Apple Silicon

Нужен Mac с Apple Silicon, Xcode Command Line Tools, Python 3.11+, Git и rustup.
Сверьте SHA-256 закрытого ZIP с [записью кандидата](../tasks/todo.md) и распакуйте
его в новый каталог на Mac вне рабочего дерева Git. Запустите команды из
распакованного корня в нативной оболочке. Укажите абсолютный путь к исходному
ZIP. Рядом с ним должен лежать входной манифест с тем же базовым именем.

```bash
(
    set -eu
    test "$(uname -s):$(uname -m)" = Darwin:arm64
    archive='/absolute/path/to/telegram-rust-0.1.0-alpha.1-94269339316a.zip'
    manifest="${archive%.zip}.manifest.json"
    evidence='target/macos-local-evidence'
    python3 -c 'from tools.check_compat import ROOT, guard; guard(ROOT)'
    mkdir -p "$evidence"
    verify_source() {
        python3 - "$archive" "$manifest" <<'PY'
import stat
import sys
from pathlib import Path
from tools.source_bundle import MAX_ARCHIVE, MAX_MANIFEST, read_bounded, verify_bundle

archive, manifest = map(Path, sys.argv[1:])
document, files = verify_bundle(read_bounded(archive, MAX_ARCHIVE), read_bounded(manifest, MAX_MANIFEST))
for name, (data, mode) in files.items():
    path = Path(name)
    if path.is_symlink() or path.read_bytes() != data or stat.S_IMODE(path.stat().st_mode) != mode:
        raise SystemExit(f"Source differs from archive: {name}")
print(document["commit"], document["archive_sha256"], len(files))
PY
    }
    verify_source > "$evidence/source-before.log"
    rustup toolchain install 1.89.0 --profile minimal --no-self-update
    rustup toolchain install 1.100.0 --profile minimal --component rustfmt,clippy --no-self-update
    cargo +1.100.0 fetch --locked
    export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=4
    export CARGO_BUILD_TARGET=aarch64-apple-darwin
    python3 -m unittest discover -s tools/tests -v > "$evidence/python.log" 2>&1
    python3 tools/schema.py check > "$evidence/schema.log" 2>&1
    for version in 1.89.0 1.100.0; do
        rustup run "$version" rustc -Vv > "$evidence/rustc-$version.log"
        grep -Fx 'host: aarch64-apple-darwin' "$evidence/rustc-$version.log"
        python3 tools/check_compat.py --toolchain "$version"
        if [ "$version" = 1.100.0 ]; then
            cargo +1.100.0 fmt --all -- --check > "$evidence/fmt.log" 2>&1
            cargo +1.100.0 clippy --locked --workspace --all-features --all-targets \
                --target-dir "target/compat-$version" -- -D warnings > "$evidence/clippy.log" 2>&1
        fi
        python3 -c 'from tools.check_compat import ROOT, guard; guard(ROOT)'
        cargo "+$version" clean --target-dir "target/compat-$version" --profile dev
    done
    RUSTUP_TOOLCHAIN=1.100.0 python3 tools/source_bundle.py check "$archive" \
        --manifest "$manifest" --consumer --target-dir target/macos-relocated-consumer \
        > "$evidence/consumer.log" 2>&1
    verify_source > "$evidence/source-after.log"
    cmp "$evidence/source-before.log" "$evidence/source-after.log"
    python3 -c 'from tools.check_compat import ROOT, guard; guard(ROOT)'
    cargo +1.100.0 clean --target-dir target/macos-relocated-consumer --profile dev
)
```

Сохраните `macos-local-evidence`, оба каталога `compat-*-evidence` и
`macos-relocated-consumer-evidence` с `consumer.Cargo.lock`. Для каждой версии Rust
ожидаются 28 пройденных стадий, а в `rustc -Vv` — host `aarch64-apple-darwin`.
Отдельный lock потребителя можно проверить аудитом на доверенном компьютере.

MSRV относится к проверенному исходному кандидату и его Cargo.lock. Новый
compatible upstream-релиз может потребовать более новый Rust; Cargo учитывает
rust-version при разрешении, но независимый внешний lock также нужно проверять.
Повышение MSRV требует явной записи в release notes и повторения матрицы.
Общий gate Q3 остаётся открытым до всех native-ячеек и реестровой упаковки.
Локальная [репетиция закрытых пакетов](PRIVATE-DISTRIBUTION.md) уже прошла на
Windows/Rust 1.100.0; обе macOS-ячейки и удалённый CI ещё не подтверждены.
