# Builds the static library and runs the plain-C tests. CARGO_TARGET_DIR is honoured, so the
# library is found wherever cargo puts it.
ROOT := $(abspath ../..)
TARGET_DIR := $(or $(CARGO_TARGET_DIR),$(ROOT)/target)
LIB := $(TARGET_DIR)/release/libtparquet_capi.a
CFLAGS := -O2 -Wall -Wextra -Werror -pthread -I include
LDLIBS := -lpthread -ldl -lm

.PHONY: lib c-test
lib:
	cargo build --release -p tparquet-capi

c-test: lib
	$(CC) $(CFLAGS) tests/c/smoke.c $(LIB) $(LDLIBS) -o $(TARGET_DIR)/pcp-smoke
	$(CC) $(CFLAGS) tests/c/api.c $(LIB) $(LDLIBS) -o $(TARGET_DIR)/pcp-api
	$(TARGET_DIR)/pcp-smoke
	$(TARGET_DIR)/pcp-api
