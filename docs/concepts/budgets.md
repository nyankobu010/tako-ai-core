# Budgets

`tako-runtime` ships an in-memory `BudgetTracker` that's consulted both
*before* every provider call (using `LlmProvider::estimate_cost_usd`)
and *after* (reconciling against the response's actual usage).

```python
budget = tako.Budget(
    max_usd_per_request=5.0,
    max_usd_per_day=500.0,
    max_usd_per_tenant_per_day={"acme": 100.0, "beta": 1000.0},
)
client = tako.Client(providers=[...], budget=budget)
```

A `BudgetExhausted` error propagates when:

- The estimated cost would push a single request over `max_usd_per_request`.
- The reconciled cost would push the day's spend over `max_usd_per_day`.
- The reconciled cost would push a tenant over their per-tenant cap.

## BudgetBackend

`BudgetTracker` delegates to a `BudgetBackend`:

```rust
#[async_trait]
pub trait BudgetBackend: Send + Sync + 'static + Debug {
    async fn current_usage(&self, tenant_id: &str) -> Result<TenantUsage, TakoError>;
    async fn record(&self, tenant_id: &str, usd: f64, tokens: u64) -> Result<(), TakoError>;
}
```

The default `InMemoryBudgetBackend` is fine for single-process
deployments. Phase 4 will add a Redis-backed implementation for
multi-tenant SaaS scenarios.

## Cost estimation

Providers expose `estimate_cost_usd(messages: &[Message])`. Most provider
crates use the per-million-token rates from their `Capabilities`
struct; you can override these at builder time:

```python
provider = tako.providers.OpenAI(
    model="gpt-5",
    api_key="...",
    capabilities=tako.Capabilities(
        max_context_tokens=128_000,
        usd_per_input_mtok=2.50,
        usd_per_output_mtok=10.00,
        ...
    ),
)
```

Estimation is conservative: it assumes the worst-case output length up
to `max_tokens` (or 1024 if unset).
