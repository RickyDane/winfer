"""Offline bridge tests: no Azure account or SDK installation required."""
import importlib.util
import base64
import contextlib
import io
import json
import sys
import types
from pathlib import Path
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location("bridge", Path(__file__).parents[1] / "plugins/cosmos-db/bridge.py")
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.client = MagicMock()
        self.container = self.client.get_database_client.return_value.get_container_client.return_value
        self.base = {"database": "db", "container": "items"}

    def test_lists_are_bounded(self):
        self.client.list_databases.return_value = ({"id": str(i)} for i in range(1000))
        self.assertEqual(len(bridge.execute({"action": "databases", "max_items": 3}, self.client)["databases"]), 3)

    def test_query_parameters_and_continuation(self):
        pages = iter([[{"id": "a"}]])
        class Pages:
            continuation_token = "next"
            def __iter__(self): return self
            def __next__(self): return next(pages)
        query = self.container.query_items.return_value
        query.by_page.return_value = Pages()
        args = {**self.base, "action": "query", "query": "SELECT * FROM c WHERE c.id=@id", "parameters": [{"name": "@id", "value": "a"}], "continuation": "previous"}
        result = bridge.execute(args, self.client)
        self.assertEqual(result, {"items": [{"id": "a"}], "continuation": "next"})
        self.assertTrue(self.container.query_items.call_args.kwargs["enable_cross_partition_query"])
        query.by_page.assert_called_once_with(continuation_token="previous")

    def test_point_read_and_delete_partition_key(self):
        for action in ["read", "delete"]:
            bridge.execute({**self.base, "action": action, "id": "1", "partition_key": ["tenant", "region"]}, self.client)
        self.container.read_item.assert_called_once_with(item="1", partition_key=["tenant", "region"])
        self.container.delete_item.assert_called_once_with(item="1", partition_key=["tenant", "region"])

    def test_write_dispatch(self):
        for action in ["create", "upsert", "replace"]:
            method = getattr(self.container, action + "_item")
            method.return_value = {"id": "1", "_etag": "tag"}
            result = bridge.execute({**self.base, "action": action, "id": "1", "item": {"id": "1"}}, self.client)
            self.assertEqual(result, {"id": "1", "etag": "tag"})
            self.assertEqual(method.call_args.kwargs["body"], {"id": "1"})

    def test_authentication_and_safe_errors(self):
        sdk = MagicMock()
        identity = MagicMock()
        modules = {"azure": types.ModuleType("azure"), "azure.cosmos": sdk, "azure.identity": identity}
        for auth in ["connection-string", "entra"]:
            request = {"action": "databases", "auth": auth, "key": "COSMOS_TEST", "endpoint": "https://account.documents.azure.com"}
            client = sdk.CosmosClient.from_connection_string.return_value if auth == "connection-string" else sdk.CosmosClient.return_value
            client.list_databases.return_value = []
            with patch.dict(sys.modules, modules), patch.dict("os.environ", {"COSMOS_TEST": "AccountKey=secret"}), patch.object(sys, "argv", ["bridge.py", base64.b64encode(json.dumps(request).encode()).decode()]), contextlib.redirect_stdout(io.StringIO()) as output:
                bridge.main()
            self.assertEqual(json.loads(output.getvalue()), {"databases": []})
        self.assertEqual(sdk.CosmosClient.from_connection_string.call_args.args[0], "AccountKey=secret")
        self.assertEqual(sdk.CosmosClient.call_args.kwargs["credential"], identity.DefaultAzureCredential.return_value)
        self.assertTrue(identity.DefaultAzureCredential.call_args.kwargs["exclude_interactive_browser_credential"])
        client.list_databases.side_effect = RuntimeError("AccountKey=secret")
        with patch.dict(sys.modules, modules), patch.object(sys, "argv", ["bridge.py", base64.b64encode(json.dumps(request).encode()).decode()]), contextlib.redirect_stdout(io.StringIO()) as output:
            bridge.main()
        self.assertNotIn("secret", output.getvalue())
        self.assertIn("error", json.loads(output.getvalue()))


if __name__ == "__main__":
    unittest.main()
