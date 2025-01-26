miri:
    ./x build compiler
    ./x build miri

    cargo install --path ./src/tools/miri/cargo-miri --force \
    --target-dir ./build/cargo-miri-install \
    --bin cargo-miri \
    --root ./build/host/stage2 \
    --locked

    cd /home/swli/rust-isolation/lib_demo && cargo miri setup

cargo-miri:
    cargo install --path ./src/tools/miri/cargo-miri --force --debug \
    --target-dir ./build/cargo-miri-install \
    --bin cargo-miri \
    --root ./build/host/stage3 \
    --locked


