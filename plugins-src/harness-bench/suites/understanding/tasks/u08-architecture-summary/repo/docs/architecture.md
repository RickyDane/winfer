# Architecture (2023)

- Entry point: `courier.main:main`.
- Storage: Redis (`RedisStore`) in production, memory in tests.
- Delivery: 3 retries (`[delivery] retries`), exponential backoff.
- Middleware: messages pass through auth -> metrics -> dedupe, in the order listed in the config.
- Handlers: order.created, order.cancelled, user.signup, refund.issued.
