# Recipe: Amazon Bedrock

Bedrock uses the AWS default credential chain (env vars, profile, IRSA,
IMDS). The provider also supports streaming via `ConverseStream`.

## Single-shot

```python
import os
import tako

provider = tako.providers.Bedrock(
    model="anthropic.claude-3-5-sonnet-20240620-v1:0",
    region=os.getenv("AWS_REGION", "us-east-1"),
)

agent = tako.SingleAgent(provider=provider, max_steps=4)
result = await agent.run("In one sentence: what is an octopus?")
```

The provider id surfaces as `bedrock:<model-id>`. Bedrock model ids look
like `<vendor>.<family>-<size>-<date>-v<version>`.

## Streaming

`tako.providers.Bedrock` advertises `supports_streaming: true`, so the
orchestrator's stream path Just Works (when streaming lands in the
orchestrator in Phase 3, this provider will already be wired).

For now, you can drive the underlying provider directly:

```rust
use tako_core::{ChatRequest, LlmProvider, Message, Principal};
let mut stream = provider
    .stream(&Principal::anonymous(), ChatRequest::new("model", vec![Message::user("hi")]))
    .await?;
while let Some(chunk) = stream.next().await {
    // chunk is ChatChunk::Delta or ChatChunk::End
}
```

## Cross-region failover

Bedrock is region-scoped. To failover, build providers per region:

```python
us = tako.providers.Bedrock(model="anthropic.claude-3-5-sonnet-20240620-v1:0", region="us-east-1")
eu = tako.providers.Bedrock(model="anthropic.claude-3-5-sonnet-20240620-v1:0", region="eu-west-1")
client = tako.Client(providers=[us, eu])
```

## VPC-private endpoints

Pass `endpoint_url=` to bypass the public Bedrock endpoint:

```python
provider = tako.providers.Bedrock(
    model="anthropic.claude-3-5-sonnet-20240620-v1:0",
    endpoint_url="https://bedrock-runtime.us-east-1.amazonaws.com",
)
```

This is also how the wiremock-style integration tests work — point
`endpoint_url` at a smithy-compatible mock server.

## Pin a profile

```python
provider = tako.providers.Bedrock(
    model="anthropic.claude-3-5-sonnet-20240620-v1:0",
    profile_name="my-team-bedrock",
)
```
