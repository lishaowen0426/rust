

cargo-miri:
    cargo install --path ./src/tools/miri/cargo-miri --force --debug \
    --target-dir ./build/cargo-miri-install \
    --bin cargo-miri \
    --root ./build/host/stage3 \
    --locked

compiler:
    ./x build compiler miri rustdoc
    cargo install --path ./src/tools/miri/cargo-miri --force --debug \
    --target-dir ./build/cargo-miri-install \
    --bin cargo-miri \
    --root ./build/host/stage3 \
    --locked


