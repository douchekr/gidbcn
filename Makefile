TARGET   := armv7-unknown-linux-gnueabihf
BINARY   := gidbcn
# 배포 대상 (환경변수로 덮어쓰기 가능: make deploy PI_HOST=pi@192.168.0.x)
PI_HOST  ?= pi@raspberrypi.local
PI_PATH  ?= /opt/kkuepark/gidbcn/$(BINARY)

.PHONY: build test build-pi deploy setup-cross

build:
	cargo build

test:
	cargo test

# ── 크로스컴파일 ─────────────────────────────────────────────────────────────
#
# 방법 A (권장): cross — Docker 기반, 시스템 패키지 불필요
#   1회 설치: cargo install cross --git https://github.com/cross-rs/cross
#   빌드:     make build-pi
#
# 방법 B: 네이티브 크로스툴체인
#   1회 설치: sudo apt install gcc-arm-linux-gnueabihf
#             rustup target add $(TARGET)
#   빌드:     make build-pi USE_CROSS=0
#
USE_CROSS ?= 1

# ring crate의 C/어셈블리 빌드에 필요한 ARM C 컴파일러 지정
# cross(방법 A)는 Docker 컨테이너에서 자동 처리되므로 불필요
# 네이티브 크로스툴체인(방법 B) 사용 시에만 필요
CC_ARM := arm-linux-gnueabihf-gcc

ifeq ($(USE_CROSS),1)
CARGO_CMD := cross
BUILD_ENV :=
else
CARGO_CMD := cargo
BUILD_ENV := CC_armv7_unknown_linux_gnueabihf=$(CC_ARM)
endif

build-pi:
	$(BUILD_ENV) $(CARGO_CMD) build --release --target $(TARGET)

# 빌드 후 SCP 배포
deploy: build-pi
	scp target/$(TARGET)/release/$(BINARY) $(PI_HOST):$(PI_PATH)
	@echo "배포 완료: $(PI_HOST):$(PI_PATH)"

# 크로스컴파일 사전 요구사항 안내
setup-cross:
	@echo "=== cross 설치 (방법 A — 권장) ==="
	@echo "  cargo install cross --git https://github.com/cross-rs/cross"
	@echo "  Docker 또는 Podman이 실행 중이어야 합니다."
	@echo ""
	@echo "=== 네이티브 툴체인 설치 (방법 B) ==="
	@echo "  sudo apt install gcc-arm-linux-gnueabihf"
	@echo "  rustup target add $(TARGET)"
	@echo "  make build-pi USE_CROSS=0"
