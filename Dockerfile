# select build image
FROM rust:1.56.1 AS builder

# create a new empty shell project
RUN USER=root cargo new --bin trin
WORKDIR /trin

RUN apt-get update && apt-get install clang -y

# copy over manifests and source to build image
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
COPY ./src ./src 
COPY ./trin-cli ./trin-cli
COPY ./trin-core ./trin-core 
COPY ./trin-history ./trin-history 
COPY ./trin-state ./trin-state 
COPY ./ethportal-peertest ./ethportal-peertest 

# build for release
RUN cargo build --all --release

# final base
FROM rust:1.56.1

# copy build artifact from build stage
COPY --from=builder /trin/target/release/trin .
COPY --from=builder /trin/target/release/trin-cli .

ENV RUST_LOG=debug

ENTRYPOINT ["./trin"]
