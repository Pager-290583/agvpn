# AGVPN — клиент AdGuard VPN для Linux (Flatpak, Rust)

Нативное приложение на GTK 4 / libadwaita, повторяющее приложение **AdGuard VPN для macOS** (v2.10).
Ставится одним файлом `.flatpak` на любой дистрибутив с Flatpak.

Это переписанная на **Rust** (gtk4-rs + libadwaita-rs) версия прежнего Python-приложения AGVPN.
App ID, формат настроек, статистики и данных CLI не изменились, поэтому новая версия ставится поверх
старой и подхватывает вход в аккаунт, настройки, исключения и статистику.

> Неофициальный проект, не связан с компанией AdGuard. Нужен аккаунт AdGuard VPN.

## Как это устроено

Протокол и API AdGuard VPN закрыты, поэтому приложение — графическая оболочка над
**официальным AdGuard VPN CLI** (`adguardvpn-cli`) для Linux:

```
┌──────────── Flatpak-песочница ─────────────┐        ┌──────────── хост ─────────────────────┐
│ GTK4/libadwaita UI (Rust)                  │        │ adguardvpn-cli (от пользователя):     │
│  ├ controller: опрос статуса, конфиг,      │ flatpak│   status, list-locations, config,     │
│  │  исключения, аккаунт                    │ -spawn │   site-exclusions, license, login     │
│  ├ tray (StatusNotifierItem + dbusmenu)    │ --host │                                       │
│  └ статистика по /sys/class/net/tun*       │ ─────► │ pkexec agvpn-helper (от root):        │
│                                            │        │   connect / disconnect / Kill Switch  │
└────────────────────────────────────────────┘        └───────────────────────────────────────┘
```

* VPN-сервису нужны сетевое пространство имён хоста, `/dev/net/tun` и root — внутри песочницы
  их нет, поэтому CLI запускается на хосте через `flatpak-spawn --host`
  (разрешение `--talk-name=org.freedesktop.Flatpak`).
* Все вызовы CLI выполняются по очереди в фоновом потоке (`util::Worker`), результаты
  возвращаются в главный цикл GLib; интерфейс никогда не ждёт CLI.
* CLI проприетарный, распространять его нельзя: в Flatpak он подключён как **extra-data** —
  скачивается с GitHub AdGuard на машине пользователя при установке и проверяется по SHA-256.
  Распаковывает архив `apply_extra` — маленькая программа на Rust из этого же крейта
  (`src/bin/apply_extra.rs`). Новые версии CLI приложение может скачать само (канал release/beta/nightly).
* Подключение, отключение и Kill Switch выполняет скрипт `helper/agvpn-helper` от root
  через `pkexec` (графический запрос пароля). Если polkit-агент недоступен — через `sudo -A`
  с вводом пароля в окне приложения. Опция «Подключаться без пароля» ставит помощник в
  `/usr/local/libexec/agvpn` и правило polkit/sudoers только для вашего пользователя.
  ⚠️ Это удобно, но ослабляет защиту: любая программа, работающая от вашего имени, сможет
  запускать VPN-сервис от root с данными из вашего домашнего каталога (как при `sudo -E
  adguardvpn-cli`, который рекомендует сам AdGuard). По умолчанию режим выключен.
* `agvpn-helper` намеренно остался POSIX-скриптом: он исполняется на хосте от root, и его
  проще проверить глазами, чем бинарник; он не зависит от библиотек песочницы.

## Функции (macOS → Linux)

| macOS AdGuard VPN | AGVPN для Linux |
|---|---|
| Кнопка подключения, статус, время подключения | ✅ |
| Локации: все / самые быстрые / сохранённые, поиск, пинг, виртуальные локации | ✅ |
| Вход через браузер, почта+пароль, 2FA | ✅ (через `adguardvpn-cli login` в псевдотерминале) |
| Онбординг, соглашения, согласие на телеметрию | ✅ |
| Исключения сайтов: Общий/Выборочный режим, поддомены, популярные сервисы, импорт/экспорт | ✅ |
| Исключения подсетей (CIDR) | ✅ |
| DNS: по умолчанию, популярные, свои (IP, DoH, DoT, DoQ, sdns) | ✅ |
| Kill Switch | ✅ nftables-таблица `inet agvpn_killswitch`, опция «разрешить локальную сеть» |
| Режим VPN / SOCKS5 (порт, адрес, логин/пароль) | ✅ |
| Протокол Авто / HTTP/2 / QUIC, постквантовая криптография | ✅ |
| Статистика: трафик, время, топ локаций, периоды | ✅ (хранится локально) |
| Аккаунт: тариф, устройства, срок, остаток трафика, выход | ✅ |
| Значок в строке меню → значок в трее с меню «Подключиться к…» | ✅ (KDE, Cinnamon, XFCE, waybar; в GNOME — с расширением AppIndicator) |
| Уведомления, автозапуск, автоподключение, тема, язык (RU/EN) | ✅ (автозапуск через Background-портал) |
| Экспорт логов и сведений о системе, сообщить об ошибке, FAQ | ✅ |
| Экспорт/импорт/сброс настроек | ✅ |
| Проверка и установка обновлений | ✅ обновления CLI; само приложение обновляет Flatpak |
| **Исключения для приложений, статистика по приложениям** | ❌ CLI для Linux умеет исключать только сайты, IP и подсети |
| Системное расширение, аппаратное ускорение, Strict VPN routing | не применимо к Linux |

## Установка

### Готовый bundle

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user agvpn-x86_64.flatpak      # runtime GNOME 50 подтянется с Flathub
flatpak run io.github.agvpn.AGVPN
```

### Сборка локально

```sh
./build.sh           # собрать и установить для текущего пользователя
./build.sh bundle    # плюс файл agvpn-<arch>.flatpak
```

Сборка идёт в GNOME SDK 50 с расширением `org.freedesktop.Sdk.Extension.rust-stable`
(`--install-deps-from=flathub` поставит их сам). Cargo собирает без сети: все зависимости
перечислены в `flatpak/cargo-sources.json` с контрольными суммами, flatpak-builder скачивает
их сам. После изменения зависимостей список обновляет `build.sh`
(или `python3 flatpak/cargo-sources.py Cargo.lock flatpak/cargo-sources.json`).

### Требования к хост-системе

* `pkexec` (polkit) или `sudo`; модуль ядра `tun` (`sudo modprobe tun`, если его нет);
* для Kill Switch — `nft` (nftables).

## Разработка

Rust и `-devel`-пакеты на хосте не нужны: `dev.sh` запускает cargo внутри GNOME SDK.
Артефакты сборки (`target`, кэш cargo, состояние flatpak-builder, локальный репозиторий)
хранятся в `~/.cache/agvpn-rs` (переменная `AGVPN_BUILD_CACHE`), чтобы папка проекта на
Яндекс.Диске оставалась маленькой.

```sh
./dev.sh build
./dev.sh test        # тесты парсеров вывода CLI, исключений, i18n, запуска процессов
./dev.sh clippy --all-targets
./dev.sh run         # окно приложения (CLI ищется как в Flatpak, либо AGVPN_CLI=/путь)
```

Переменные: `AGVPN_CLI=/путь/к/adguardvpn-cli`, `AGVPN_DEBUG=1`.

## Где что хранится

| Что | Где (в Flatpak) |
|---|---|
| Данные CLI (сессия, конфиг, логи) | `~/.var/app/io.github.agvpn.AGVPN/data/adguardvpn-cli` |
| Настройки приложения | `~/.var/app/io.github.agvpn.AGVPN/config/settings.json` |
| Статистика | `~/.var/app/io.github.agvpn.AGVPN/data/stats.json` |
| Лог приложения | `~/.var/app/io.github.agvpn.AGVPN/cache/agvpn.log` |

## Структура

```
src/
  main.rs, app.rs     точка входа; Adw.Application, трей, уведомления, выход, автообновление CLI
  controller.rs       состояние приложения и все обращения к CLI, сигналы для UI
  cli.rs              запуск CLI на хосте, pty для входа, pkexec/sudo + askpass
  parsers.rs          разбор текстового вывода CLI (+ тесты)
  settings.rs         settings.json;  stats.rs — статистика;  updater.rs — загрузка CLI с GitHub
  tray.rs             StatusNotifierItem + com.canonical.dbusmenu через Gio
  i18n.rs, i18n_ru.rs перевод (макрос tr!, русские формы множественного числа)
  ui/                 окно и страницы: home, exclusions, stats, settings_page, onboarding
  bin/apply_extra.rs  распаковка CLI при установке Flatpak
helper/               agvpn-helper — привилегированные операции (sh, root)
data/                 иконки, CSS, .desktop, metainfo
flatpak/              манифест, генератор cargo-sources.json
```
