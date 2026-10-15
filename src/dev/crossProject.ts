/**
 * Dev-only fake projects for the Cross-Project Knowledge plugin (plugins/cross-project): answers
 * the shell commands the plugin runs through `vault_exec` with the source of three made-up
 * projects that share a NuGet model package: the package itself, an ASP.NET web app with a React
 * front end, and a .NET MAUI mobile app. Open the dev server with `?xp` to install the plugin with
 * the three projects linked, or `?xp=empty` to link them yourself.
 */

const MODE = new URLSearchParams(location.search).get("xp");
export const XP_DEMO = MODE !== null;

const HOME = "/Users/maya";
const ROOTS = { contracts: `${HOME}/code/shop-contracts`, web: `${HOME}/code/shop-web`, mobile: `${HOME}/code/shop-mobile` };

const PROJECTS: Record<string, Record<string, string>> = {
  [ROOTS.contracts]: {
    "src/Acme.Shop.Contracts/Acme.Shop.Contracts.csproj": `<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><PackageId>Acme.Shop.Contracts</PackageId><Version>3.2.0</Version></PropertyGroup></Project>`,
    "src/Acme.Shop.Contracts/Orders/OrderDto.cs": `using System.Text.Json.Serialization;
namespace Acme.Shop.Contracts.Orders;

public sealed class OrderDto
{
    [JsonPropertyName("id")] public Guid Id { get; init; }
    public string CustomerName { get; set; } = "";
    public decimal Total { get; set; }
    public string Currency { get; set; } = "EUR";
    public DateTimeOffset PlacedAt { get; set; }
    public OrderStatus Status { get; set; }
    public List<OrderLineDto> Lines { get; set; } = new();
    public AddressDto ShippingAddress { get; set; } = new();
}

public record OrderLineDto(string Sku, string Name, [property: JsonPropertyName("qty")] int Quantity, decimal UnitPrice);

public enum OrderStatus { Pending, Paid, Shipped, Cancelled }

public record CreateOrderRequest
{
    public required List<OrderLineDto> Lines { get; init; }
    public required AddressDto ShippingAddress { get; init; }
    public string? CouponCode { get; init; }
}
`,
    "src/Acme.Shop.Contracts/Customers/AddressDto.cs": `namespace Acme.Shop.Contracts.Customers;
public class AddressDto
{
    public string Street { get; set; } = "";
    public string City { get; set; } = "";
    public string PostalCode { get; set; } = "";
    public string Country { get; set; } = "";
}
`,
    "src/Acme.Shop.Contracts/Catalog/ProductDto.cs": `namespace Acme.Shop.Contracts.Catalog;
public record ProductDto(string Sku, string Name, decimal Price, int Stock, string ImageUrl);
public record PagedResult<T>(IReadOnlyList<T> Items, int Page, int Total);
`,
  },
  [ROOTS.web]: {
    "Acme.Shop.Web/Acme.Shop.Web.csproj": `<Project Sdk="Microsoft.NET.Sdk.Web"><ItemGroup><PackageReference Include="Acme.Shop.Contracts" Version="3.2.0" /><PackageReference Include="Serilog.AspNetCore" Version="8.0.1" /><PackageReference Include="Swashbuckle.AspNetCore" Version="6.6.2" /></ItemGroup></Project>`,
    "Acme.Shop.Web/Controllers/OrdersController.cs": `using Acme.Shop.Contracts.Orders;
using Microsoft.AspNetCore.Mvc;
namespace Acme.Shop.Web.Controllers;

[ApiController]
[Route("api/[controller]")]
public class OrdersController(IOrderService orders) : ControllerBase
{
    [HttpGet("{id:guid}")]
    public async Task<ActionResult<OrderDto>> Get(Guid id, CancellationToken ct) => Ok(await orders.Find(id, ct));

    [HttpGet]
    public Task<IEnumerable<OrderDto>> Mine([FromQuery] int page = 1) => orders.ForCurrentUser(page);

    [HttpPost]
    [ProducesResponseType(typeof(OrderDto), 201)]
    public async Task<IActionResult> Create(CreateOrderRequest request) => Created("", await orders.Create(request));

    [HttpPost("{id:guid}/cancel")]
    public async Task<ActionResult<OrderDto>> Cancel(Guid id) => Ok(await orders.Cancel(id));
}
`,
    "Acme.Shop.Web/Controllers/CatalogController.cs": `using Acme.Shop.Contracts.Catalog;
using Microsoft.AspNetCore.Mvc;
namespace Acme.Shop.Web.Controllers;

[ApiController]
[Route("api/catalog")]
public class CatalogController(ICatalog catalog) : ControllerBase
{
    [HttpGet("products")]
    public Task<PagedResult<ProductDto>> Products([FromQuery] string? q, [FromQuery] int page = 1) => catalog.Search(q, page);

    [HttpGet("products/{sku}")]
    public Task<ProductDto> Product(string sku) => catalog.Get(sku);
}
`,
    "Acme.Shop.Web/Program.cs": `var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();
var account = app.MapGroup("/api/account");
account.MapGet("/address", (IAccount acc) => TypedResults.Ok<AddressDto>(acc.Address()));
account.MapPut("/address", (AddressDto address, IAccount acc) => acc.Save(address));
app.MapControllers();
app.Run();
`,
    "ClientApp/package.json": `{ "name": "shop-web-client", "private": true, "dependencies": { "react": "^18.3.1", "@tanstack/react-query": "^5.51.0" } }`,
    "ClientApp/src/api/types.ts": `// Hand-written mirror of Acme.Shop.Contracts — keep in sync!
export interface OrderDto {
  id: string;
  customerName: string;
  total: string;
  currency: string;
  placedAt: string;
  status: "Pending" | "Paid" | "Shipped" | "Cancelled";
  lines: OrderLineDto[];
}

export interface OrderLineDto { sku: string; name: string; qty: number; unitPrice: number; }

export interface ProductDto { sku: string; name: string; price: number; stock: number; imageUrl: string; }
`,
    "ClientApp/src/api/orders.ts": `import type { OrderDto } from "./types";

export async function getOrder(id: string): Promise<OrderDto> {
  const res = await fetch(\`/api/orders/\${id}\`);
  return (await res.json()) as OrderDto;
}

export async function cancelOrder(id: string) {
  const res = await fetch(\`/api/orders/\${id}/cancel\`, { method: "POST" });
  return (await res.json()) as OrderDto;
}
`,
  },
  [ROOTS.mobile]: {
    "Acme.Shop.Mobile/Acme.Shop.Mobile.csproj": `<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><UseMaui>true</UseMaui></PropertyGroup><ItemGroup><PackageReference Include="Acme.Shop.Contracts" Version="3.1.0" /><PackageReference Include="CommunityToolkit.Mvvm" Version="8.2.2" /><PackageReference Include="Serilog.AspNetCore" Version="8.0.1" /></ItemGroup></Project>`,
    "Acme.Shop.Mobile/Services/ApiRoutes.cs": `namespace Acme.Shop.Mobile.Services;
public static class ApiRoutes
{
    public const string Orders = "api/orders";
    public const string Products = "api/catalog/products";
    public const string Address = "api/account/address";
}
`,
    "Acme.Shop.Mobile/Services/ShopApi.cs": `using System.Net.Http.Json;
using Acme.Shop.Contracts.Catalog;
using Acme.Shop.Contracts.Customers;
using Acme.Shop.Contracts.Orders;
namespace Acme.Shop.Mobile.Services;

public class ShopApi(HttpClient http)
{
    public Task<OrderDto?> GetOrder(Guid id) => http.GetFromJsonAsync<OrderDto>($"{ApiRoutes.Orders}/{id}");

    public Task<List<OrderDto>?> MyOrders() => http.GetFromJsonAsync<List<OrderDto>>(ApiRoutes.Orders);

    public async Task<OrderDto?> PlaceOrder(IEnumerable<OrderLineDto> lines, AddressDto address)
    {
        var response = await http.PostAsJsonAsync(ApiRoutes.Orders, new CreateOrderRequest { Lines = lines.ToList(), ShippingAddress = address });
        response.EnsureSuccessStatusCode();
        return await response.Content.ReadFromJsonAsync<OrderDto>();
    }

    public Task<PagedResult<ProductDto>?> Search(string q) => http.GetFromJsonAsync<PagedResult<ProductDto>>($"{ApiRoutes.Products}?q={q}");

    public Task SaveAddress(AddressDto address) => http.PutAsJsonAsync(ApiRoutes.Address, address);

    public Task<HttpResponseMessage> Wishlist() => http.GetAsync("api/wishlist");
}
`,
    "Acme.Shop.Mobile/ViewModels/OrderViewModel.cs": `using Acme.Shop.Contracts.Orders;
namespace Acme.Shop.Mobile.ViewModels;
public partial class OrderViewModel(ShopApi api)
{
    public OrderDto? Order { get; private set; }
    public bool CanCancel => Order?.Status == OrderStatus.Pending;
    public async Task Load(Guid id) => Order = await api.GetOrder(id);
}
`,
  },
};

const files = new Map<string, string>();
let counter = 0;

const ok = (output: string) => ({ code: 0, timedOut: false, output });
const delay = <T>(ms: number, value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), ms));

async function packed(text: string) {
  const stream = new Blob([new TextEncoder().encode(text)]).stream().pipeThrough(new CompressionStream("gzip"));
  const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  const b64 = btoa(bin);
  const name = `/var/folders/xp/tmp.${++counter}`;
  files.set(name, b64);
  return `${b64.length} ${name}\n`;
}

/** Links the three demo projects on first start (unless `?xp=empty`). */
export function seedCrossProject() {
  const key = "winfer.plugin.cross-project";
  if (MODE === "empty" || localStorage.getItem(key)) return;
  localStorage.setItem(
    key,
    JSON.stringify({
      systems: [{ id: "sys-shop", name: "Shop (web + mobile)", projects: [{ root: ROOTS.contracts, name: "Contracts (NuGet)" }, { root: ROOTS.web, name: "Web app" }, { root: ROOTS.mobile, name: "Mobile app" }] }],
      activeId: "sys-shop",
    }),
  );
}

export function crossProjectExec(command: string, cwd: string): Promise<{ code: number; timedOut: boolean; output: string }> | { code: number; timedOut: boolean; output: string } {
  if (command.startsWith('echo "$HOME"')) return ok(`${HOME}\n`);
  if (command === "pwd -P") return PROJECTS[cwd] || cwd.startsWith(HOME) ? ok(`${cwd}\n`) : { code: 1, timedOut: false, output: "no such directory" };
  if (command.startsWith("f=$(mktemp)")) {
    const project = PROJECTS[cwd] ?? {};
    const dump = Object.entries(project)
      .map(([path, text]) => `\n@@FILE ${path}\n${text}`)
      .join("");
    return delay(600 + Math.random() * 900, null).then(() => packed(dump).then(ok));
  }
  const chunk = /^tail -c \+(\d+) '([^']+)' \| head -c (\d+)$/.exec(command);
  if (chunk) return ok(files.get(chunk[2])?.slice(Number(chunk[1]) - 1, Number(chunk[1]) - 1 + Number(chunk[3])) ?? "");
  if (command.startsWith("rm -f")) return ok("");
  if (command.startsWith("b=$(command -v graphify")) return ok("\n");
  if (command.startsWith("grep -nwE")) return ok("");
  return ok("");
}
