"""Private SDK bridge. Credentials arrive only via the plugin vault environment."""
import base64
import itertools
import json
import os
import sys


def execute(request, client):
    action = request["action"]
    limit = min(100, max(1, int(request.get("max_items", 25))))
    if action == "databases":
        return {"databases": [d["id"] for d in itertools.islice(client.list_databases(), limit)]}
    database = client.get_database_client(request["database"])
    if action == "containers":
        return {"containers": [{"id": c["id"], "partitionKey": c.get("partitionKey")} for c in itertools.islice(database.list_containers(), limit)]}
    container = database.get_container_client(request["container"])
    if action == "query":
        options = {"query": request["query"], "parameters": request.get("parameters", []), "max_item_count": limit}
        if "partition_key" in request:
            options["partition_key"] = request["partition_key"]
        else:
            options["enable_cross_partition_query"] = True
        pages = container.query_items(**options).by_page(continuation_token=request.get("continuation"))
        page = next(pages, None)
        items = list(page) if page is not None else []
        return {"items": items, "continuation": pages.continuation_token}
    if action == "read":
        return {"item": container.read_item(item=request["id"], partition_key=request["partition_key"])}
    if action == "create":
        item = container.create_item(body=request["item"])
    elif action == "upsert":
        item = container.upsert_item(body=request["item"])
    elif action == "replace":
        item = container.replace_item(item=request["id"], body=request["item"])
    elif action == "delete":
        container.delete_item(item=request["id"], partition_key=request["partition_key"])
        return {"deleted": request["id"]}
    else:
        raise ValueError("Unsupported action")
    return {"id": item["id"], "etag": item.get("_etag")}


def main():
    request = json.loads(base64.b64decode(sys.argv[1]))
    try:
        from azure.cosmos import CosmosClient
        from azure.identity import DefaultAzureCredential
    except ImportError:
        print(json.dumps({"error": "Install dependencies: python -m pip install azure-cosmos azure-identity"}))
        return
    try:
        if request["auth"] == "connection-string":
            client = CosmosClient.from_connection_string(os.environ[request["key"]], connection_timeout=15, timeout=45, retry_total=0)
            credential = None
        else:
            credential = DefaultAzureCredential(exclude_interactive_browser_credential=True)
            client = CosmosClient(request["endpoint"], credential=credential, connection_timeout=15, timeout=45, retry_total=0)
        try:
            result = execute(request, client)
        finally:
            client.close()
            if credential:
                credential.close()
        output = json.dumps(result, ensure_ascii=True)
        if len(output) > 16000:
            output = json.dumps({"error": "Result exceeds 16 KB. Select fewer/smaller fields or reduce max_items; no result was returned."})
        print(output)
    except Exception as error:
        # SDK exceptions can contain headers, tokens or connection details. Never echo them.
        status = getattr(error, "status_code", None)
        print(json.dumps({"error": "Cosmos request failed" + (f" (HTTP {status})" if isinstance(status, int) else "") + ". Check connectivity, credentials, data-plane RBAC and request arguments on the Cosmos DB page. A timed-out write may have committed; read before retrying."}))


if __name__ == "__main__":
    main()
