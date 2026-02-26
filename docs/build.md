# 빌드 가이드

## 타겟 디바이스

라즈베리파이 2 Model B+ (ARMv7-A Cortex-A7, 1GB RAM)

- Rust 타겟: `armv7-unknown-linux-gnueabihf`
- 바이너리 배포 경로: `/home/pi/gidbcn`
- 데이터 경로: `/opt/kkuepark/gidbcn/`

---

## 크로스컴파일 사전 설치

### 방법 A — cross (권장)

Docker 또는 Podman 기반. 시스템 패키지 불필요.

```bash
# cross 설치 (1회)
cargo install cross --git https://github.com/cross-rs/cross

# Docker가 실행 중이어야 함
sudo systemctl start docker
```

### 방법 B — 네이티브 툴체인

```bash
# 크로스 컴파일러 설치 (1회)
sudo apt install gcc-arm-linux-gnueabihf

# Rust 타겟 추가 (1회)
rustup target add armv7-unknown-linux-gnueabihf
```

> **주의**: `rustls`의 crypto 백엔드인 `ring` v0.17은 C/ARM32 어셈블리를 포함합니다.
> 방법 B는 Makefile이 `CC_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-gcc`를 자동으로 설정하므로 별도 조치 불필요.
> 방법 A(`cross`)는 Docker 컨테이너 내부에서 처리되므로 역시 불필요.

---

## 빌드

```bash
# 방법 A (cross)
make build-pi

# 방법 B (네이티브 툴체인)
make build-pi USE_CROSS=0

# 결과물 위치
target/armv7-unknown-linux-gnueabihf/release/gidbcn
```

release 빌드는 자동으로 strip 됨 (`.cargo/config.toml`).

---

## 배포

```bash
# 기본 (hostname: raspberrypi)
make deploy

# IP 직접 지정
make deploy PI_HOST=pi@192.168.0.x

# 배포 경로 변경
make deploy PI_HOST=pi@192.168.0.x PI_PATH=/home/pi/bin/gidbcn
```

내부적으로 `build-pi` 후 scp로 전송.

---

## Pi에서 초기 설정

```bash
# 데이터 디렉토리 생성
sudo mkdir -p /opt/kkuepark/gidbcn
sudo chown pi:pi /opt/kkuepark/gidbcn

# 바이너리 첫 실행 → config.json 템플릿 자동 생성
./gidbcn

# config.json 편집
nano /opt/kkuepark/gidbcn/config.json
# kis_api.app_key, app_secret, hts_id, telegram.bot_token 입력

# 다시 실행
./gidbcn
```

---

## 서비스 등록 (systemd)

```ini
# /etc/systemd/system/gidbcn.service
[Unit]
Description=gidbcn finance signal bot
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/home/pi/gidbcn
Restart=on-failure
RestartSec=10
User=pi
WorkingDirectory=/home/pi

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable gidbcn
sudo systemctl start gidbcn
sudo journalctl -u gidbcn -f   # 로그 확인
```

---

## 호스트에서 로컬 빌드/테스트

```bash
make build    # 호스트 아키텍처 debug 빌드
make test     # 단위 테스트 실행
```

---

## 런타임 요구사항 분석

| 항목 | 현황 | 판단 |
|---|---|---|
| 메모리 | current_thread 런타임, 수 KB JSON, 단일 HTTP 클라이언트 | 예상 RSS 20~40MB (1GB 중 여유) |
| CPU | 5분 주기 폴링, long-polling 대기 중 idle | Cortex-A7으로 충분 |
| TLS | rustls 사용 (OpenSSL 없음) | 크로스컴파일 의존성 없음 |
| 32비트 | f64, i64 ARMv7에서 정상 동작 | 문제 없음 |
| uuid v4 | /dev/urandom 사용 | Linux ARM 정상 동작 |

### 주의 사항

teloxide + `current_thread` 호환성 이슈 (GitHub #366): teloxide 내부에서 `spawn_blocking`을 사용하는 경우 single-thread 런타임에서 hang 가능. 배포 전 Pi에서 명령어 1개 응답 PoC 검증 필수.
