# Cosmos DB

Azure Cosmos DB **for NoSQL** only (not MongoDB, Cassandra, Gremlin or Table).

1. Install Python 3 and `python -m pip install azure-cosmos azure-identity`.
   On macOS/Linux use `python3` for both installation and execution.
2. Open **Cosmos DB** from the palette and add a named connection:
   - **Connection string**: `AccountEndpoint=https://…;AccountKey=…;` is saved in
     the OS keychain and injected into the SDK process, never given to agents.
   - **Microsoft integrated (Entra identity)**: enter the HTTPS account endpoint.
     [DefaultAzureCredential](https://learn.microsoft.com/en-us/azure/developer/python/sdk/authentication/credential-chains)
     uses your existing `az login`, managed identity or configured workload identity.
     This is Entra authentication, not Windows/SQL Server SSPI. Interactive browser
     authentication is disabled. Grant Cosmos **data-plane RBAC** to that identity.
3. Agents use `cosmos_list`, then `cosmos_read` (databases, containers, SELECT or
   point read), and `cosmos_write` (create, upsert, replace, delete).

**Ask before writes** defaults to on per connection. Every write approval includes
the calling agent, database, container, operation and full item. Turning it off
requires confirmation; reads never ask. Use read-only Azure credentials/RBAC for
server-enforced protection. Agents cannot change this toggle through plugin tools.

Queries accept parameters and return one SDK page plus a continuation token. Pass
the same query/parameters with that token for the next page. Use projections and
`max_items` (1–100) to keep results below 16 KB. Database/container lists are capped;
they are not exhaustive for accounts with more than 100 entries. Requests are
limited to 12 KB. Point reads/deletes need an explicit partition key (including
`null` or an array for hierarchical keys); item writes include partition-key fields
in the document. Upsert can overwrite existing data. No schema or bulk writes.

Writes are not automatically retried. A timed-out write may already have committed:
read its item before retrying. Large Windows requests temporarily stage item data
(not credentials) in a self-removing script in the plugin folder; interruption may
leave a `.request-*.ps1` file, which can be deleted when no request is running.

Offline checks: `pnpm exec vitest run scripts/cosmos-db.test.mjs` and
`python -B scripts/cosmos_bridge_test.py`. A dependency-free tool smoke check is
available as `node scripts/cosmos-db.check.mjs`. Real authentication and database access
require your account, network/firewall access and appropriate permissions.
