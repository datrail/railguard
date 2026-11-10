# The stand-in's and the driver's image: Python with gRPC, and the ExternalCallout stubs generated
# from the vendored .proto (the `proto` build context).
FROM python:3.12-slim@sha256:05cda9777409a9c3ffddd94a4c476b79f0769a0b4857f0c7ed9226b6800b0d6f
COPY requirements.txt /tmp/
RUN pip install --no-cache-dir --require-hashes --only-binary :all: -r /tmp/requirements.txt
COPY --from=proto external_callout.proto /proto/
RUN install -d /stubs && python -m grpc_tools.protoc -I/proto --python_out=/stubs --grpc_python_out=/stubs external_callout.proto
ENV PYTHONPATH=/stubs
# Not root, as Apigee and the agent aren't.
USER 10002
