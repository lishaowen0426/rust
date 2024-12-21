miri:
    ./x build miri

    cargo install --path ./src/tools/miri/cargo-miri --force \
    --target-dir ./build/cargo-miri-install \
    --bin cargo-miri \
    --locked

    rustup run stage2 cargo install --path ./src/tools/miri/cargo-miri --force
