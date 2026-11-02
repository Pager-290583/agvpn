#!/usr/bin/env python3
"""Prepare the Flatpak installer assets and notes for a successful GitHub Actions build."""
import argparse
import configparser
import hashlib
import json
from pathlib import Path
import re
import shutil
import tomllib

APP_ID = 'io.github.agvpn.AGVPN'
ASSETS = ('agvpn.flatpakref', 'agvpn.flatpakrepo', 'INSTALL.txt', 'build-info.json', 'SHA256SUMS')


def prepare_release(root, site, output, repository, sha, run_number, run_id):
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise ValueError('Invalid GitHub repository')
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('Expected the complete source commit SHA')
    if run_number < 1 or run_id < 1:
        raise ValueError('Expected positive GitHub run identifiers')
    with (root / 'Cargo.toml').open('rb') as fh:
        version = tomllib.load(fh)['package']['version']
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version):
        raise ValueError('Expected a stable application version')
    owner, name = repository.split('/')
    site_url = f'https://{owner.lower()}.github.io/{name}'
    repo_url = f'{site_url}/repo'
    for filename, section in [('agvpn.flatpakref', 'Flatpak Ref'), ('agvpn.flatpakrepo', 'Flatpak Repo')]:
        config = configparser.ConfigParser(interpolation=None)
        with (site / filename).open(encoding='utf-8') as fh:
            config.read_file(fh)
        if config.get(section, 'Url') != repo_url:
            raise ValueError(f'{filename} does not point to the published repository')
        if filename.endswith('.flatpakref'):
            if config.get(section, 'Name') != APP_ID or config.get(section, 'Branch') != 'master':
                raise ValueError('Unexpected Flatpak application or branch')
            if config.get(section, 'RuntimeRepo') != 'https://dl.flathub.org/repo/flathub.flatpakrepo':
                raise ValueError('Unexpected runtime repository')

    tag = f'v{version}-build.{run_number}'
    release_url = f'https://github.com/{repository}/releases/download/{tag}'
    build_url = f'https://github.com/{repository}/actions/runs/{run_id}'
    output.mkdir(parents=True, exist_ok=True)
    for filename in ASSETS[:2]:
        shutil.copyfile(site / filename, output / filename)
    (output / 'INSTALL.txt').write_text(f'''AGVPN {version} — установка для Linux

Скачайте agvpn.flatpakref и откройте его в GNOME Software или KDE Discover.
Архитектура (x86_64 / aarch64) выбирается автоматически.

Установка через терминал:
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./agvpn.flatpakref

Запуск:
flatpak run {APP_ID}

Обновление:
flatpak update {APP_ID}

agvpn.flatpakref — сетевой установщик Flatpak. Он подключает репозиторий
приложения; установка и последующие обновления загружаются из него.
Официальный AdGuard VPN CLI и среда GNOME загружаются при установке.
Файл не является автономным пакетом для установки без интернета.
Дополнительный agvpn.flatpakrepo предназначен для подключения репозитория.

Проверка скачанных файлов: sha256sum -c SHA256SUMS
Если включено подключение без пароля, обновите помощник в расширенных настройках.

Сборка установщика: {tag}
Исходный код: {sha}
Проверки сборки: {build_url}
Репозиторий приложения: {repo_url}
''', encoding='utf-8')
    (output / 'build-info.json').write_text(json.dumps({
        'version': version, 'tag': tag, 'commit': sha, 'workflow_run': run_id,
        'architectures': ['x86_64', 'aarch64'], 'repository': repository,
        'flatpak_repository': repo_url, 'installer_type': 'flatpakref',
    }, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    checksums = ''.join(f'{hashlib.sha256((output / name).read_bytes()).hexdigest()}  {name}\n' for name in ASSETS[:-1])
    (output / 'SHA256SUMS').write_text(checksums, encoding='utf-8')
    (output / 'release-notes.txt').write_text(f'''Готовый установщик AGVPN для Linux: **x86_64 и aarch64**.

Скачайте [agvpn.flatpakref]({release_url}/agvpn.flatpakref) из Assets и откройте его в GNOME Software / KDE Discover.
Архитектура выбирается автоматически. Для установки нужен интернет.

Через терминал:
```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./agvpn.flatpakref
```

Файл `.flatpakref` подключает [репозиторий приложения]({site_url}/), из которого устанавливаются и обновляются сборки.
Официальный CLI AdGuard VPN загружается при установке. Нужен аккаунт AdGuard VPN.

В Assets также есть `INSTALL.txt`, `agvpn.flatpakrepo`, `build-info.json` и `SHA256SUMS`.
При использовании подключения без пароля обновите помощник в расширенных настройках приложения.

Версия приложения: **{version}**. Коммит: `{sha}`.
[Успешная сборка и тесты]({build_url}).
''', encoding='utf-8')
    return {'tag': tag, 'title': f'AGVPN {version} — сборка {run_number}'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--site-dir', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--sha', required=True)
    parser.add_argument('--run-number', type=int, required=True)
    parser.add_argument('--run-id', type=int, required=True)
    args = parser.parse_args()
    metadata = prepare_release(Path(__file__).resolve().parents[1], args.site_dir, args.output_dir,
                               args.repository, args.sha, args.run_number, args.run_id)
    for key, value in metadata.items():
        print(f'{key}={value}')


if __name__ == '__main__':
    main()
