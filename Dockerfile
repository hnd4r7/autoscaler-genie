FROM cgr.dev/chainguard/static
COPY --chown=nonroot:nonroot ./autoscaler-genie /app/
EXPOSE 8080
ENTRYPOINT ["/app/autoscaler-genie"]
