import { describe, expect, it } from "vitest";
import { factsFromDump, linkSystem, normPath, pathScore, resolveUrlExpr, typeNames } from "../plugins/cross-project/index.js";

const dump = (files) => Object.entries(files).map(([path, text]) => `\n@@FILE ${path}\n${text}`).join("");

const contracts = dump({
  "src/Acme.Contracts/Acme.Contracts.csproj": `<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><PackageId>Acme.Contracts</PackageId><Version>2.1.0</Version></PropertyGroup></Project>`,
  "src/Acme.Contracts/Orders/OrderDto.cs": `
using System.Text.Json.Serialization;
namespace Acme.Contracts.Orders;

/// <summary>An order, as the API sends it. { not a brace }</summary>
public sealed class OrderDto
{
    [JsonPropertyName("id")] public Guid Id { get; init; }
    public string Customer { get; set; } = "";
    public decimal Total { get; set; }
    public List<OrderLineDto> Lines { get; set; } = new();
    public OrderStatus Status { get; set; }
    [JsonIgnore] public string Secret { get; set; }
    public static OrderDto Empty => new();
    public string Describe() { return $"{Customer} {{ }}"; }
}

public record OrderLineDto(string Sku, [property: JsonPropertyName("qty")] int Quantity);

public record CreateOrderRequest
{
    public required string Customer { get; init; }
    public List<OrderLineDto> Lines { get; init; } = [];
}

public enum OrderStatus { Pending, [EnumMember(Value = "shipped")] Shipped, Cancelled = 9 }
`,
});

const web = dump({
  "Acme.Web/Acme.Web.csproj": `<Project Sdk="Microsoft.NET.Sdk.Web"><ItemGroup><PackageReference Include="Acme.Contracts" Version="2.1.0" /><PackageReference Include="Newtonsoft.Json" Version="13.0.3" /></ItemGroup></Project>`,
  "Acme.Web/Controllers/OrdersController.cs": `
using Acme.Contracts.Orders;
using Microsoft.AspNetCore.Mvc;
namespace Acme.Web.Controllers;

[ApiController]
[Route("api/[controller]")]
public class OrdersController : ControllerBase
{
    private readonly IOrderService _orders;

    [HttpGet("{id:guid}")]
    public async Task<ActionResult<OrderDto>> Get(Guid id, CancellationToken ct) => Ok(await _orders.Find(id, ct));

    [HttpGet]
    public Task<IEnumerable<OrderDto>> List([FromQuery] int page) => _orders.List(page);

    [HttpPost]
    [ProducesResponseType(typeof(OrderDto), 201)]
    public async Task<IActionResult> Create([FromServices] IOrderService svc, CreateOrderRequest request) => Created("", await svc.Create(request));
}
`,
  "Acme.Web/Program.cs": `
var app = WebApplication.Create(args);
var health = app.MapGroup("/api/health");
health.MapGet("/ping", () => TypedResults.Ok<PingDto>(new PingDto()));
app.Run();
public class PingDto { public string Status { get; set; } }
`,
  "web/src/api/types.ts": `
export interface OrderDto {
  id: string;
  customer: string;
  total: string;      // wrong: the API sends a number
  status?: "Pending" | "shipped";
}
`,
});

const mobile = dump({
  "Acme.Mobile/Acme.Mobile.csproj": `<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><PackageReference Include="Acme.Contracts" /><PackageReference Include="Newtonsoft.Json" Version="13.0.1" /></ItemGroup></Project>`,
  "Directory.Packages.props": `<Project><ItemGroup><PackageVersion Include="Acme.Contracts" Version="2.0.0" /></ItemGroup></Project>`,
  "Acme.Mobile/Services/OrderApi.cs": `
using System.Net.Http.Json;
using Acme.Contracts.Orders;
namespace Acme.Mobile.Services;

public static class Routes { public const string Orders = "api/orders"; }

public class OrderApi(HttpClient http)
{
    public Task<OrderDto?> Get(Guid id) => http.GetFromJsonAsync<OrderDto>($"{Routes.Orders}/{id}");

    public async Task<OrderDto?> Create(string customer)
    {
        var response = await http.PostAsJsonAsync(Routes.Orders, new CreateOrderRequest { Customer = customer });
        return await response.Content.ReadFromJsonAsync<OrderDto>();
    }

    public Task Missing() => http.DeleteAsync("api/carts/7");
}
`,
  "Acme.Mobile/Views/OrderPage.xaml.cs": `
namespace Acme.Mobile.Views;
public partial class OrderPage { private OrderStatus _status; }
`,
});

describe("cross-project scanner", () => {
  it("normalizes routes and URL expressions", () => {
    expect(normPath("https://api.acme.dev/api/orders/{id:guid}?x=1")).toEqual(["api", "orders", "{}"]);
    expect(normPath("${base}/orders/:id")).toEqual(["orders", "{}"]);
    expect(resolveUrlExpr('$"{Routes.Orders}/{id}"', new Map([["Orders", "api/orders"]]))).toBe("api/orders/{id}");
    expect(resolveUrlExpr('BaseUrl + "/orders/" + id', new Map())).toBe("{}//orders/{}");
    expect(typeNames("Task<ActionResult<List<OrderDto>>>")).toEqual(["OrderDto"]);
    expect(pathScore(["api", "orders", "{}"], ["orders", "{}"])).toBeGreaterThan(0);
    expect(pathScore(["api", "orders"], ["api", "carts"])).toBe(0);
    expect(pathScore(["api", "{}"], ["api", "x"])).toBe(0);
  });

  it("extracts C# models, controllers and client calls", () => {
    const lib = factsFromDump(contracts);
    const order = lib.types.find((t) => t.name === "OrderDto");
    expect(order.namespace).toBe("Acme.Contracts.Orders");
    expect(order.fields.map((f) => f.json || f.name)).toEqual(["id", "Customer", "Total", "Lines", "Status"]);
    expect(lib.types.find((t) => t.name === "OrderLineDto").fields).toEqual([
      { name: "Sku", json: "", type: "string" },
      { name: "Quantity", json: "qty", type: "int" },
    ]);
    expect(lib.types.find((t) => t.name === "OrderStatus").fields.map((f) => f.json || f.name)).toEqual(["Pending", "shipped", "Cancelled"]);
    expect(lib.provides).toEqual([expect.objectContaining({ name: "Acme.Contracts", version: "2.1.0" })]);

    const w = factsFromDump(web);
    expect(w.endpoints.map((e) => `${e.method} ${e.path} ${e.request} -> ${e.response}`)).toEqual([
      "GET api/Orders/{id:guid}  -> OrderDto",
      "GET api/Orders  -> OrderDto",
      "POST api/Orders CreateOrderRequest -> OrderDto",
      "GET /api/health/ping  -> PingDto",
    ]);

    const m = factsFromDump(mobile);
    expect(m.calls.map((c) => `${c.method} ${c.segs.join("/")} ${c.request} -> ${c.response}`)).toEqual([
      "GET api/orders/{}  -> OrderDto",
      "POST api/orders CreateOrderRequest -> OrderDto",
      "DELETE api/carts/7  -> ",
    ]);
    expect(m.packages.find((p) => p.name === "Acme.Contracts").version).toBe("2.0.0");
  });

  it("links contracts, endpoints and packages across projects", () => {
    const sys = linkSystem([
      { root: "/c", name: "contracts", facts: factsFromDump(contracts) },
      { root: "/w", name: "web", facts: factsFromDump(web) },
      { root: "/m", name: "mobile", facts: factsFromDump(mobile) },
    ]);
    const order = sys.contracts.find((c) => c.name === "OrderDto");
    expect(order.package).toBe("Acme.Contracts");
    expect(order.strength).toBe("strong");
    expect(order.roots.sort()).toEqual(["/c", "/m", "/w"]);
    expect(order.per["/w"].counts.ser).toBeGreaterThan(0);
    expect(order.per["/m"].counts.deser).toBeGreaterThanOrEqual(2);
    // The hand-written TS mirror in the web project drifts from the package's model.
    const byKey = Object.fromEntries(order.fields.rows.map((r) => [r.key, r.issue]));
    expect(byKey).toMatchObject({ total: "type", lines: "missing", id: null, customer: null });

    expect(sys.contracts.find((c) => c.name === "CreateOrderRequest")).toMatchObject({ package: "Acme.Contracts", strength: "strong" });
    expect(sys.contracts.find((c) => c.name === "OrderStatus")?.roots.sort()).toEqual(["/c", "/m"]);

    const acme = sys.packages.find((p) => p.name === "Acme.Contracts");
    expect(acme).toMatchObject({ drift: true, contracts: 3 });
    expect(acme.versions.sort()).toEqual(["2.0.0", "2.1.0"]);

    expect(sys.apis.map((a) => `${a.method} ${a.path} <- ${a.clients.map((c) => c.root).join(",")}`)).toEqual(["POST /api/orders <- /m", "GET /api/orders/{} <- /m"]);
    expect(sys.apis.find((a) => a.method === "POST").clients[0]).toMatchObject({ root: "/m", request: ["CreateOrderRequest"] });
    expect(sys.apis.find((a) => a.method === "GET" && a.path === "/api/orders/{}").clients).toHaveLength(1);
    expect(sys.unmatched).toEqual([expect.objectContaining({ method: "DELETE", path: "api/carts/7" })]);
  });

  it("links a TypeScript API with Kotlin, Swift and Dart clients", () => {
    const server = dump({
      "package.json": `{ "name": "shop-api", "dependencies": { "express": "^4.19.0", "zod": "3.23.0" } }`,
      "src/models.ts": `
export interface Product { id: number; title: string; priceCents: number; tags: string[]; }
export type CartItem = { productId: number; quantity: number };
`,
      "src/routes.ts": `
import { Product } from "./models";
router.get("/api/products/:id", async (req, res) => { res.json(product as Product); });
app.post('/api/cart', (req, res) => res.sendStatus(201));
`,
    });
    const android = dump({
      "app/build.gradle.kts": `dependencies { implementation("com.squareup.retrofit2:retrofit:2.11.0") }`,
      "app/src/main/java/shop/Api.kt": `
interface ShopApi {
    @GET("api/products/{id}")
    suspend fun product(@Path("id") id: Int): Product

    @POST("api/cart")
    suspend fun add(@Body item: CartItem): Response<Unit>
}
@Serializable
data class Product(val id: Int, val title: String, @SerialName("priceCents") val price: Long)
data class CartItem(val productId: Int, val quantity: Int)
`,
    });
    const ios = dump({
      "Shop/Product.swift": `
struct Product: Codable, Identifiable {
    let id: Int
    let title: String
    let price: Double
    var isCheap: Bool { price < 5 }
    enum CodingKeys: String, CodingKey { case id, title, price = "priceCents" }
}
func load(id: Int) async throws -> Product {
    let (data, _) = try await URLSession.shared.data(from: URL(string: "https://shop.dev/api/products/\\(id)")!)
    return try JSONDecoder().decode(Product.self, from: data)
}
`,
    });
    const flutter = dump({
      "pubspec.yaml": "name: shop_app\ndependencies:\n  dio: ^5.4.0\n",
      "lib/product.dart": `
class Product {
  final int id;
  final String title;
  @JsonKey(name: 'priceCents') final int price;
  Product({required this.id, required this.title, required this.price});
  factory Product.fromJson(Map<String, dynamic> json) => _$ProductFromJson(json);
}
Future<Product> fetchProduct(int id) async {
  final res = await dio.get('/api/products/$id');
  return Product.fromJson(res.data);
}
`,
    });
    const sys = linkSystem([
      { root: "/api", name: "api", facts: factsFromDump(server) },
      { root: "/android", name: "android", facts: factsFromDump(android) },
      { root: "/ios", name: "ios", facts: factsFromDump(ios) },
      { root: "/flutter", name: "flutter", facts: factsFromDump(flutter) },
    ]);
    const product = sys.contracts.find((c) => c.name === "Product");
    expect(product.roots.sort()).toEqual(["/android", "/api", "/flutter", "/ios"]);
    const rows = Object.fromEntries(product.fields.rows.map((r) => [r.key, r]));
    expect(rows.pricecents.cells["/android"]).toMatchObject({ name: "price", json: "priceCents" });
    expect(rows.pricecents.cells["/ios"]).toMatchObject({ name: "price", json: "priceCents" });
    expect(rows.tags.missing.sort()).toEqual(["/android", "/flutter", "/ios"]);
    expect(rows.ischeap).toBeUndefined();

    const get = sys.apis.find((a) => a.method === "GET");
    expect(get.path).toBe("/api/products/{}");
    expect(get.clients.map((c) => c.root).sort()).toEqual(["/android", "/flutter", "/ios"]);
    const post = sys.apis.find((a) => a.method === "POST");
    expect(post.clients).toEqual([expect.objectContaining({ root: "/android", request: ["CartItem"] })]);
    expect(sys.contracts.find((c) => c.name === "CartItem")?.fields.problems).toBe(0);
  });
});

