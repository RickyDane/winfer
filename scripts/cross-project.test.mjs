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

// Swagger/OpenAPI clients against controllers with inherited routes and route constants.
const csServer = dump({
    "Api/Controllers/ApiControllerBase.cs": `[ApiController]\n[Route("api/[controller]")]\npublic abstract class ApiControllerBase : ControllerBase {}`,
    "Api/Routes.cs": `public static class ApiRoutes { public const string Customers = "api/customers"; }`,
    "Api/Controllers/OrdersController.cs": `
public class OrdersController : ApiControllerBase {
  [HttpGet("{id}", Name = "GetOrder"), Authorize]
  [ProducesResponseType(typeof(OrderDto), StatusCodes.Status200OK)]
  public async Task<IActionResult> Get(int id) => Ok();
  [Route("search")]
  [HttpPost]
  public ActionResult<List<OrderDto>> Search([FromBody] SearchRequest q) => null;
}
[Area("Admin")]
public class CustomersController : Controller {
  [HttpGet(ApiRoutes.Customers + "/{id}")] public CustomerDto Get(int id) => null;
}`,
});
const csClients = dump({
    "Mobile/Api/ApiClient.g.cs": `
public partial class OrdersClient {
  public virtual System.Threading.Tasks.Task<OrderDto> GetAsync(int id) { return GetAsync(id, System.Threading.CancellationToken.None); }
  public virtual async System.Threading.Tasks.Task<OrderDto> GetAsync(int id, System.Threading.CancellationToken cancellationToken)
  {
      var client_ = _httpClient;
      using (var request_ = new System.Net.Http.HttpRequestMessage())
      {
          request_.Method = new System.Net.Http.HttpMethod("GET");
          var urlBuilder_ = new System.Text.StringBuilder();
          if (!string.IsNullOrEmpty(_baseUrl)) urlBuilder_.Append(_baseUrl);
          // Operation Path: "api/Orders/{id}"
          urlBuilder_.Append("api/Orders/");
          urlBuilder_.Append(System.Uri.EscapeDataString(ConvertToString(id, System.Globalization.CultureInfo.InvariantCulture)));
          var response_ = await client_.SendAsync(request_, cancellationToken);
          var objectResponse_ = await ReadObjectResponseAsync<OrderDto>(response_, headers_, cancellationToken).ConfigureAwait(false);
      }
  }
  public virtual async System.Threading.Tasks.Task<System.Collections.Generic.ICollection<OrderDto>> SearchAsync(SearchRequest body, System.Threading.CancellationToken cancellationToken)
  {
      using (var request_ = new System.Net.Http.HttpRequestMessage())
      {
          var json_ = System.Text.Json.JsonSerializer.SerializeToUtf8Bytes(body, JsonSerializerSettings);
          request_.Method = new System.Net.Http.HttpMethod("POST");
          var urlBuilder_ = new System.Text.StringBuilder();
          urlBuilder_.Append(BaseUrl != null ? BaseUrl.TrimEnd('/') : "").Append("/api/Orders/search?");
          if (page != null) urlBuilder_.Append(System.Uri.EscapeDataString("page") + "=").Append("&");
      }
  }
}
public partial class OrderDto { [System.Text.Json.Serialization.JsonPropertyName("id")] public int Id { get; set; } }`,
    "Mobile/Kiota/CustomersRequestBuilder.cs": `
public class ItemRequestBuilder : BaseRequestBuilder {
  public ItemRequestBuilder(Dictionary<string, object> p, IRequestAdapter a) : base(a, "{+baseurl}/Admin/api/customers/{id}{?expand*}", p) {}
  public async Task<CustomerDto?> GetAsync(Action<RequestConfiguration<DefaultQueryParameters>>? requestConfiguration = default, CancellationToken cancellationToken = default) {
    var requestInfo = ToGetRequestInformation(requestConfiguration);
    return await RequestAdapter.SendAsync<CustomerDto>(requestInfo, CustomerDto.CreateFromDiscriminatorValue, default, cancellationToken);
  }
}`,
    "Mobile/OpenAPIs/swagger.yaml": `openapi: 3.0.1
info:
  title: Api
  description: >
    multi
    line
paths:
  /api/Orders/{id}:
    get:
      operationId: GetOrder
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/OrderDto'
  /api/Orders/{id}/cancel:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: '#/components/schemas/CancelRequest' }
      responses:
        '204':
          description: none
components:
  schemas:
    CancelRequest:
      type: object
      properties:
        reason:
          type: string
        tags:
          type: array
          items:
            type: string
`,
});

// The same API served and called from Go, Rust and Zig.
const goProject = dump({
    "go.mod": "module github.com/acme/api\n\ngo 1.22\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.10.0\n\tgithub.com/acme/models v0.3.0\n)\n",
    "main.go": `package main
import (
  "net/http"
  "github.com/gin-gonic/gin"
  models "github.com/acme/models/orders"
)
type OrderDto struct {
  ID    string  \`json:"id"\`
  Total float64 \`json:"total,omitempty"\`
  Lines []OrderLine \`json:"lines"\`
  secret string
  Skip string \`json:"-"\`
}
type CreateOrder struct { Customer string \`json:"customer"\` }
func main() {
  r := gin.Default()
  api := r.Group("/api")
  api.GET("/orders/:id", getOrder)
  api.POST("/orders", func(c *gin.Context) {
    var req CreateOrder
    if err := c.ShouldBindJSON(&req); err != nil { return }
    c.JSON(http.StatusCreated, OrderDto{ID: "1"})
  })
  mux := http.NewServeMux()
  mux.HandleFunc("DELETE /api/orders/{id}", h.deleteOrder)
}
func getOrder(c *gin.Context) {
  order := OrderDto{}
  c.JSON(http.StatusOK, order)
}
`,
    "client.go": `package main
func fetch(base string) (*OrderDto, error) {
  req, _ := http.NewRequestWithContext(ctx, http.MethodGet, base+"/api/orders/"+id, nil)
  var out OrderDto
  json.NewDecoder(resp.Body).Decode(&out)
  return &out, nil
}`,
});
const rustProject = dump({
    "Cargo.toml": `[package]\nname = "acme-mobile"\nversion = "0.2.0"\n\n[dependencies]\nserde = { version = "1.0", features = ["derive"] }\nreqwest = "0.12"\nacme-models.workspace = true\n\n[dependencies.tokio]\nversion = "1.38"\n\n[workspace.dependencies]\nacme-models = "0.3.0"\n`,
    "src/api.rs": `use acme_models::orders::OrderDto;
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrder<'a> {
    pub customer: &'a str,
    #[serde(rename = "qty")] pub quantity: Option<u32>,
    #[serde(skip)] pub cache: Vec<u8>,
}
pub async fn get_order<'a>(client: &'a reqwest::Client, base: &str, id: &str) -> Result<OrderDto, reqwest::Error> {
    client.get(format!("{}/api/orders/{}", base, id)).send().await?.json::<OrderDto>().await
}
pub async fn create(client: &reqwest::Client, body: &CreateOrder<'_>) -> reqwest::Result<OrderDto> {
    let c = 'x';
    client.post(format!("{base}/api/orders")).json(body).send().await?.json::<OrderDto>().await
}
async fn list(State(db): State<Db>) -> Json<Vec<OrderDto>> { todo!() }
async fn add(Json(input): Json<CreateOrder>) -> impl IntoResponse { todo!() }
pub fn app() -> Router { Router::new().route("/api/orders", get(list).post(add)) }
#[get("/api/health")]
async fn health() -> impl Responder { HttpResponse::Ok() }
`,
});
const zigProject = dump({
    "build.zig.zon": `.{ .name = .acme_cli, .version = "0.1.0", .dependencies = .{ .httpz = .{ .url = "https://github.com/karlseguin/http.zig/archive/refs/tags/v0.24.0.tar.gz", .hash = "x" }, .models = .{ .path = "../models" } } }`,
    "src/main.zig": `const std = @import("std");
const models = @import("models");
pub const OrderDto = struct {
    id: []const u8,
    total: f64 = 0,
    lines: []const OrderLine,
    pub fn describe(self: OrderDto) void { _ = self; }
};
const Status = enum { pending, shipped };
fn getOrder(alloc: std.mem.Allocator, client: *std.http.Client, base: []const u8, id: []const u8) !OrderDto {
    const url = try std.fmt.allocPrint(alloc, "{s}/api/orders/{s}", .{ base, id });
    _ = try client.fetch(.{ .method = .GET, .location = .{ .url = try std.fmt.allocPrint(alloc, "{s}/api/orders/{s}", .{ base, id }) }, .response_storage = .{ .dynamic = &body } });
    const parsed = try std.json.parseFromSlice(OrderDto, alloc, body.items, .{});
    return parsed.value;
}
pub fn routes(router: anytype) void { router.post("/api/orders", createOrder, .{}); }
fn createOrder(req: *httpz.Request, res: *httpz.Response) !void {
    const input = try req.json(CreateOrder);
    const out = OrderDto{ .id = "1", .lines = &.{} };
    try res.json(out, .{});
}
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

  it("finds controllers with inherited routes, route constants and mixed attributes", () => {
    const s = factsFromDump(csServer);
    expect(s.endpoints.map((e) => `${e.method} ${e.path} ${e.request} -> ${e.response}`)).toEqual([
      "GET api/Orders/{id}  -> OrderDto",
      "POST api/Orders/search SearchRequest -> OrderDto",
      "GET api/customers/{id}  -> CustomerDto",
    ]);
  });

  it("reads NSwag and Kiota clients and OpenAPI documents", () => {
    const c = factsFromDump(csClients);
    expect(c.calls.map((x) => `${x.method} ${x.segs.join("/")} ${x.request} -> ${x.response} @${x.file}`)).toEqual([
      "GET api/orders/{}  -> OrderDto @Mobile/Api/ApiClient.g.cs",
      "POST api/orders/search SearchRequest -> OrderDto @Mobile/Api/ApiClient.g.cs",
      "GET admin/api/customers/{}  -> CustomerDto @Mobile/Kiota/CustomersRequestBuilder.cs",
      "POST api/orders/{}/cancel CancelRequest ->  @Mobile/OpenAPIs/swagger.yaml",
    ]);
    expect(c.types.find((t) => t.name === "CancelRequest")).toMatchObject({ lang: "openapi", fields: [{ name: "reason", type: "string" }, { name: "tags", type: "string[]" }] });

    const sys = linkSystem([
      { root: "/s", name: "server", facts: factsFromDump(csServer) },
      { root: "/c", name: "client", facts: c },
    ]);
    expect(sys.apis.map((a) => `${a.method} ${a.path} <- ${a.clients.length}`)).toEqual(["GET /api/customers/{} <- 1", "GET /api/orders/{} <- 1", "POST /api/orders/search <- 1"]);
  });

  it("ignores an API's own exported OpenAPI document", () => {
    const own = dump({
      "Api/Controllers/OrdersController.cs": `[ApiController][Route("api/orders")] public class OrdersController : ControllerBase { [HttpGet("{id}")] public OrderDto Get(int id) => null; }`,
      "Api/swagger.json": JSON.stringify({ openapi: "3.0.1", paths: { "/api/orders/{id}": { get: { responses: { 200: { description: "ok" } } } } } }),
    });
    expect(factsFromDump(own).calls).toEqual([]);
  });

  it("links Go, Rust and Zig servers and clients", () => {
    const go = factsFromDump(goProject);
    expect(go.endpoints.map((e) => `${e.method} ${e.path} ${e.request} -> ${e.response}`)).toEqual([
      "GET /api/orders/:id  -> OrderDto",
      "POST /api/orders CreateOrder -> OrderDto",
      "DELETE /api/orders/{id}  -> ",
    ]);
    expect(go.types.find((t) => t.name === "OrderDto").fields.map((f) => f.json)).toEqual(["id", "total", "lines"]);
    expect(go.packages.map((p) => `${p.name}@${p.version}`)).toContain("github.com/acme/models@v0.3.0");

    const rust = factsFromDump(rustProject);
    expect(rust.endpoints.map((e) => `${e.method} ${e.path} ${e.request} -> ${e.response}`)).toEqual([
      "GET /api/health  -> ",
      "GET /api/orders  -> OrderDto",
      "POST /api/orders CreateOrder -> ",
    ]);
    expect(rust.calls.map((c) => `${c.method} ${c.segs.join("/")} ${c.request} -> ${c.response}`)).toEqual(["GET api/orders/{}  -> OrderDto", "POST api/orders CreateOrder -> OrderDto"]);
    expect(rust.types[0].fields).toEqual([
      { name: "customer", json: "", type: "&'a str" },
      { name: "quantity", json: "qty", type: "Option<u32>" },
    ]);
    expect(rust.packages.find((p) => p.name === "acme_models").version).toBe("0.3.0");

    const zig = factsFromDump(zigProject);
    expect(zig.endpoints.map((e) => `${e.method} ${e.path} ${e.request} -> ${e.response}`)).toEqual(["POST /api/orders CreateOrder -> OrderDto"]);
    expect(zig.calls.map((c) => `${c.method} ${c.segs.join("/")} -> ${c.response}`)).toEqual(["GET api/orders/{} -> OrderDto"]);
    expect(zig.packages.map((p) => `${p.name}@${p.version}`)).toEqual(["httpz@0.24.0", "models@"]);

    const sys = linkSystem([
      { root: "/go", name: "go", facts: go },
      { root: "/rs", name: "rs", facts: rust },
      { root: "/zig", name: "zig", facts: zig },
    ]);
    expect(sys.apis.map((a) => `${a.method} ${a.path} (${a.server.root}) <- ${a.clients.map((c) => c.root).join(",")}`)).toEqual([
      "POST /api/orders (/go) <- /rs",
      "GET /api/orders/{} (/go) <- /rs,/zig",
    ]);
    // A call whose path matches nothing isn't linked just because the method does.
    expect(sys.unmatched).toEqual([expect.objectContaining({ root: "/go", method: "GET" })]);
    expect(sys.contracts.find((c) => c.name === "OrderDto")).toMatchObject({ strength: "strong", problems: 0 });
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

