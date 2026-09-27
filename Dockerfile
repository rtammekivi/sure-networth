FROM rust:1.98-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src
COPY src ./src
COPY web ./web
RUN touch src/main.rs && cargo build --release --locked

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /src/target/release/sure-networth /usr/local/bin/sure-networth
USER 10001:10001
ENV NETWORTH_BIND=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
  CMD ["sure-networth", "healthcheck"]
ENTRYPOINT ["sure-networth"]
CMD ["serve"]
