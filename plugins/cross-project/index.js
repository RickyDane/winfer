// Cross-Project Knowledge: links several projects that talk to each other (say a mobile app, a
// web API and the NuGet package with the models both use) into one "system". It reads each
// project's source, finds the data contracts they share (the same model serialized on one side
// and deserialized on the other, or mirrored by hand), the HTTP endpoints one serves and another
// calls, and the packages they have in common; then shows that as a map between the projects and
// flags drift (a field missing on one side, different types, different package versions).
// Agents see the same links through agent tools (xkb_overview, xkb_contract, xkb_impact, …), and
// xkb_query asks every linked project's graphify knowledge graph at once.
//
// Files are read through `api.vault.exec` (a shell in each project folder, with no secrets).

const STORAGE = "winfer.plugin.cross-project";
const IS_WIN = /Win/i.test(globalThis.navigator?.platform ?? "") || /Windows/i.test(globalThis.navigator?.userAgent ?? "");
const REDUCED_MOTION = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
/** What agent tools return at most. */
const TOOL_OUTPUT_MAX = 12_000;
/** vault.exec keeps only the last 20,000 characters of output; chunks stay under that. */
const CHUNK = 19_000;
/** Source files bigger than this are skipped (generated code, bundles). */
const MAX_FILE = 400_000;
/** Generated API clients and OpenAPI documents may be this big. */
const MAX_GENERATED = 6_000_000;
/** Paths of files that get MAX_GENERATED. */
const BIG = "[Cc]lient|[Pp]roxy|[Ss]wagger|[Oo]pen[Aa][Pp][Ii]|[Nn][Ss]wag|[Gg]enerated|[Rr]equest[Bb]uilder|\\.json$|\\.ya?ml$|\\.nswag$";
/** At most this much source is read per project. */
const MAX_TOTAL = 40_000_000;
/** Usage sites kept per contract, project and role. */
const MAX_SITES = 25;

// Analysis ---------------------------------------------------------------------------------------
//
// Everything here is pure (text in, facts out) so it can be tested without winfer.

const LANGS = { cs: "csharp", ts: "ts", tsx: "ts", js: "ts", jsx: "ts", mjs: "ts", cjs: "ts", kt: "kotlin", java: "java", swift: "swift", dart: "dart", go: "go", rs: "rust", zig: "zig" };
export const LANG_LABELS = { csharp: "C#", ts: "TS/JS", kotlin: "Kotlin", java: "Java", swift: "Swift", dart: "Dart", go: "Go", rust: "Rust", zig: "Zig", openapi: "OpenAPI" };

/** Type names that wrap the interesting one (List<OrderDto> is about OrderDto). */
const WRAPPERS = new Set(
  (
    "List IList IEnumerable ICollection IReadOnlyList IReadOnlyCollection IAsyncEnumerable Collection ObservableCollection Array ReadonlyArray " +
    "Task ValueTask ActionResult IActionResult IResult Results TypedResults Ok Created NotFound BadRequest Promise Observable Response ResponseEntity Call Single Maybe Flow Deferred Future FutureOr Stream " +
    "Dictionary IDictionary IReadOnlyDictionary Map HashMap MutableMap Set HashSet MutableList MutableSet Optional Nullable Record Partial Readonly Required Pick Omit " +
    "String Int Integer Long Short Byte Double Float Decimal Boolean Bool Object Any Unit Void Never Unknown Date DateTime DateTimeOffset DateOnly TimeOnly TimeSpan Guid UUID Uri URL Char Number BigInteger BigDecimal Instant LocalDate LocalDateTime " +
    "HttpResponseMessage HttpResponse HttpRequest JsonElement JsonNode JsonDocument JObject JToken JArray JsonObject CancellationToken IFormFile FormData Blob File Buffer Uint8Array ArrayBuffer Data Error Exception " +
    "Int32 Int64 Int16 UInt32 UInt64 Single SByte CGFloat Codable Decodable Encodable Equatable Hashable Identifiable Serializable Parcelable Sendable " +
    "SwaggerResponse FileResponse ApiResponse IApiResponse " +
    "Vec VecDeque Box Option Result Arc Rc Cow Json Path Query State Form Extension HashMap BTreeMap BTreeSet Value Utc NaiveDate NaiveDateTime HttpResponse Responder StatusCode Time Duration Mutex RwLock Allocator ArrayList"
  ).split(" "),
);
/** Names too generic to be a shared contract even when two projects both have one. */
const STOP = new Set(
  (
    "Program Startup App AppDelegate SceneDelegate MainActivity MainApplication MainPage MainWindow AppShell Constants Config Configuration Settings AppSettings " +
    "Utils Util Helpers Helper Extensions Resources Strings Colors Theme Styles BuildConfig Module Application Base Default Index Home Router Routes Api Client Service " +
    "Assembly Properties Global Globals Test Tests Mock Fixture Main Layout Page View Component Props State Context Provider Store Action Reducer " +
    "ApiException SwaggerException FileParameter ProblemDetails ValidationProblemDetails HttpValidationProblemDetails"
  ).split(" "),
);
/** Server-side parameters that are injected, not sent in the request body. */
const INJECTED = /^(I[A-Z]\w*|\w*(Context|Service|Repository|Manager|Logger|Options|Factory|Provider|Handler|Mediator|Client)|CancellationToken|HttpContext|HttpRequest|ClaimsPrincipal|IFormFile)$/;

function blankRange(chars, from, to) {
  for (let i = from; i < to; i++) if (chars[i] !== "\n") chars[i] = " ";
}

/**
 * Blanks comments out of source (same length, newlines kept): `code` keeps string literals,
 * `masked` blanks their contents too so braces in strings don't confuse brace matching.
 */
export function lex(src, lang) {
  const code = src.split("");
  const masked = src.split("");
  const n = src.length;
  let i = 0;
  while (i < n) {
    const ch = src[i];
    const nx = src[i + 1];
    if (ch === "/" && nx === "/") {
      let j = src.indexOf("\n", i);
      if (j < 0) j = n;
      blankRange(code, i, j);
      blankRange(masked, i, j);
      i = j;
      continue;
    }
    if (ch === "/" && nx === "*") {
      let j = src.indexOf("*/", i + 2);
      j = j < 0 ? n : j + 2;
      blankRange(code, i, j);
      blankRange(masked, i, j);
      i = j;
      continue;
    }
    if (ch === '"' && nx === '"' && src[i + 2] === '"') {
      let j = src.indexOf('"""', i + 3);
      j = j < 0 ? n : j + 3;
      blankRange(masked, i + 3, Math.max(i + 3, j - 3));
      i = j;
      continue;
    }
    // Rust lifetimes ('a) and Zig/Rust labels aren't char literals.
    if (ch === "'" && (lang === "rust" || lang === "zig") && !/^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'/.test(src.slice(i, i + 14))) {
      i++;
      continue;
    }
    if (ch === "`" && lang === "go") {
      let j = src.indexOf("`", i + 1);
      if (j < 0) j = n;
      blankRange(masked, i + 1, j);
      i = j + 1;
      continue;
    }
    if (ch === '"' || ch === "'" || (ch === "`" && lang === "ts")) {
      const verbatim = lang === "csharp" && ch === '"' && (src[i - 1] === "@" || (src[i - 1] === "$" && src[i - 2] === "@"));
      let j = i + 1;
      while (j < n) {
        const d = src[j];
        if (d === "\\" && !verbatim) {
          j += 2;
          continue;
        }
        if (d === ch) {
          if (verbatim && src[j + 1] === '"') {
            j += 2;
            continue;
          }
          break;
        }
        if (d === "\n" && ch !== "`" && !verbatim) break;
        j++;
      }
      blankRange(masked, i + 1, Math.min(j, n));
      i = j + 1;
      continue;
    }
    i++;
  }
  return { code: code.join(""), masked: masked.join("") };
}

/** pos → 1-based line number. */
function lineIndex(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) starts.push(i + 1);
  return (pos) => {
    let lo = 0;
    let hi = starts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (starts[mid] <= pos) lo = mid;
      else hi = mid - 1;
    }
    return lo + 1;
  };
}

/** Index of the bracket closing the one at `open` (in masked text). */
function matchClose(s, open) {
  const o = s[open];
  const c = o === "{" ? "}" : o === "(" ? ")" : o === "[" ? "]" : ">";
  let d = 0;
  for (let i = open; i < s.length; i++) {
    const x = s[i];
    if (x === o) d++;
    else if (x === c && --d === 0) return i;
  }
  return s.length;
}

/** The text of code[from, to) with every nested {…} collapsed to "{}": a type's own members. */
function flatten(code, masked, from, to) {
  let out = "";
  let depth = 0;
  let start = from;
  for (let i = from; i < to; i++) {
    const x = masked[i];
    if (x === "{") {
      if (depth === 0) out += `${code.slice(start, i)}{`;
      depth++;
    } else if (x === "}" && depth > 0) {
      depth--;
      if (depth === 0) {
        out += "}";
        start = i + 1;
      }
    }
  }
  if (depth === 0) out += code.slice(start, to);
  return out;
}

/** Splits at separators outside of <>, (), [] and {}. */
function splitTop(text, seps = ",") {
  const out = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const x = text[i];
    if (x === "<" || x === "(" || x === "[" || x === "{") depth++;
    else if ((x === ">" && text[i - 1] !== "=") || x === ")" || x === "]" || x === "}") depth = Math.max(0, depth - 1);
    else if (depth === 0 && seps.includes(x)) {
      out.push(text.slice(start, i));
      start = i + 1;
    }
  }
  out.push(text.slice(start));
  return out.map((s) => s.trim()).filter(Boolean);
}

/** The argument expression starting at `start` (up to a top-level "," or ")"). */
function argAt(code, masked, start) {
  let depth = 0;
  let i = start;
  for (; i < masked.length; i++) {
    const x = masked[i];
    if (x === "(" || x === "[" || x === "{") depth++;
    else if (x === ")" || x === "]" || x === "}") {
      if (depth === 0) break;
      depth--;
    } else if (x === "," && depth === 0) break;
  }
  return { text: code.slice(start, i).trim(), end: i, last: masked[i] !== "," };
}

/** The interesting type names in a type expression: Task<ActionResult<List<OrderDto>>> → [OrderDto]. */
export function typeNames(str) {
  const out = [];
  for (const m of String(str ?? "").matchAll(/[A-Za-z_][\w]*(?:\.[A-Za-z_]\w*)*/g)) {
    const name = m[0].split(".").pop();
    if (name.length < 2 || !/^[A-Z]/.test(name) || WRAPPERS.has(name) || /^T[A-Z]\w*$/.test(name)) continue;
    if (!out.includes(name)) out.push(name);
  }
  return out;
}

/** A field's key for comparing across languages: OrderId, orderId and order_id are the same. */
export const normKey = (name) => String(name).toLowerCase().replace(/[_\-\s]/g, "");
const tidy = (s) => String(s ?? "").replace(/\s+/g, " ").trim();

const NUMBERS = new Set(
  "int long short byte sbyte uint ulong ushort float double decimal number integer int32 int64 int16 uint32 uint64 num bigint cgfloat int8 uint8 uint16 float32 float64 rune uintptr i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f16 f32 f64 f128 comptime_int comptime_float c_int".split(" "),
);
const STRINGS = new Set("string guid datetime datetimeoffset dateonly timeonly date uuid uri url char timespan instant localdate localdatetime zoneddatetime offsetdatetime str time naivedate naivedatetime".split(" "));

/** Rough JSON shape of a type in any of the languages: number, string, bool, array, object or "". */
export function typeFamily(type) {
  // Option<T>, *T, &'a T, ?T: the value's own shape.
  const s = String(type ?? "")
    .replace(/^(?:\s*(?:Option|Box|Arc|Rc)\s*<\s*)+([\s\S]*?)(?:\s*>)+$/, "$1")
    .replace(/^[\s*&?]*(?:'\w+\s+)?(?:mut\s+)?/, "")
    .replace(/[?!]/g, "")
    .trim();
  if (!s) return "";
  if (/^\[\]\s*(?:const\s+)?u8$|^\[:0\]\s*const\s+u8$/.test(s)) return "string";
  if (/^map\[|^(?:HashMap|BTreeMap)\b/.test(s)) return "object";
  if (/\[\]$|^\[.*\]$|^\[\]|^(I?List|IEnumerable|ICollection|IReadOnly\w*|Array|ReadonlyArray|Set|HashSet|MutableList|Collection|Iterable|Seq|Sequence|Vec|VecDeque|BTreeSet|ArrayList)\b/.test(s)) return "array";
  const base = s.replace(/<.*$/, "").split(".").pop().toLowerCase();
  if (STRINGS.has(base)) return "string";
  if (NUMBERS.has(base)) return "number";
  if (base === "bool" || base === "boolean") return "bool";
  if (["dictionary", "idictionary", "map", "record", "object", "any", "jobject", "jsonelement", "unknown", "dynamic", "hashmap"].includes(base)) return "object";
  return "";
}

const JSON_NAMES = [
  /JsonPropertyName\(\s*"([^"]+)"/,
  /JsonProperty\(\s*(?:PropertyName\s*=\s*|value\s*=\s*)?"([^"]+)"/,
  /DataMember\([^)]*Name\s*=\s*"([^"]+)"/,
  /SerializedName\(\s*(?:value\s*=\s*)?"([^"]+)"/,
  /SerialName\(\s*"([^"]+)"/,
  /Json\(\s*name\s*=\s*"([^"]+)"/,
  /JsonKey\(\s*name:\s*['"]([^'"]+)['"]/,
  /EnumMember\([^)]*Value\s*=\s*"([^"]+)"/,
  /JsonStringEnumMemberName\(\s*"([^"]+)"/,
  /serde\([^)]*\brename\s*=\s*"([^"]+)"/,
];
/** The JSON name an annotation gives a member, or "". */
function jsonName(annotations) {
  for (const re of JSON_NAMES) {
    const m = re.exec(annotations ?? "");
    if (m) return m[1];
  }
  return "";
}
const ignored = (annotations) => /\bJsonIgnore\b|\bIgnoreDataMember\b|@Transient\b|\bJsonKey\([^)]*ignore:\s*true|\bserde\([^)]*\bskip\b/.test(annotations ?? "");

/** Facts about one project, filled file by file. */
export function newFacts() {
  return {
    files: 0,
    langs: {},
    types: [],
    usages: [],
    endpoints: [],
    calls: [],
    literals: [],
    consts: new Map(),
    packages: [],
    provides: [],
    centralVersions: new Map(),
    tokens: new Map(),
    imports: new Map(),
    controllers: new Map(),
    specs: [],
  };
}

const GEN = "<((?:[^<>()]|<(?:[^<>()]|<[^<>()]*>)*>)*)>";

function addUsage(F, types, mode, file, line, via) {
  for (const type of typeof types === "string" ? typeNames(types) : types) F.usages.push({ type, mode, file, line, via });
}

const FILE_LIKE = /\.(png|jpe?g|gif|svg|webp|css|scss|[cm]?[jt]sx?|json|html?|xml|ico|woff2?|ttf|md|txt|cs|dll|sql|ya?ml|toml|lock|sh|py|rs|go|kt|swift|dart)$/i;

/** Path-like string literals and string constants ("api/orders", ORDERS = "/api/orders"). */
function scanStrings(F, file, code, line) {
  for (const m of code.matchAll(/\b([A-Za-z_]\w*)\s*(?::\s*[\w<>?]+\s*)?(?:=|:)\s*\$?@?(["'`])((?:https?:\/\/|\/)?[\w{}$.:-]*\/[^"'`\n]*)\2/g)) F.consts.set(m[1], m[3]);
  const SEG = "(?:[A-Za-z][\\w.-]*|\\{[^}\"'`\\n]*\\}|\\$\\{[^}\"'`\\n]*\\}|\\\\\\([^)\"'`\\n]*\\))";
  const re = new RegExp(`(["'\`])((?:https?://[^/"'\`\\s]+)?/?${SEG}(?:/(?:${SEG}|:\\w+)?)+)\\1`, "g");
  for (const m of code.matchAll(re)) {
    const p = m[2];
    if (/^(application|text|image|audio|video|multipart|font|model)\//i.test(p) || FILE_LIKE.test(p)) continue;
    if (F.literals.length < 4000) F.literals.push({ path: p, file, line: line(m.index) });
  }
}

/** Joins a URL expression's literal parts, filling in known constants: BaseUrl + "/orders/" + id → {}/orders/{}. */
export function resolveUrlExpr(expr, consts, info = {}) {
  let out = "";
  let first = true;
  for (const m of String(expr).matchAll(/\$?@?(["'`])((?:\\.|(?!\1)[^\\])*)\1|([A-Za-z_][\w.]*)(\s*\()?/g)) {
    if (m[1]) {
      info.known = true;
      out += m[2].replace(/\$?\{([A-Za-z_][\w.]*)\}/g, (x, name) => consts.get(name.split(".").pop()) ?? x);
    } else if (m[3] && !m[4] && !/^(new|await|this|Uri|URL|parse|string|const|let|var)$/.test(m[3])) {
      const last = m[3].split(".").pop();
      // Only names that read like a route constant: Routes.Orders, ORDERS_URL, ordersEndpoint.
      const value = /^[A-Z]/.test(m[3]) || /^[A-Z]/.test(last) || /url|path|endpoint|route|api/i.test(last) ? consts.get(last) : undefined;
      if (value !== undefined) info.known = true;
      out += value ?? (first ? "{}/" : "{}");
    }
    first = false;
  }
  return out;
}

/** A route as comparable segments: "/api/orders/{id:int}?x=1" → ["api", "orders", "{}"]. */
export function normPath(raw) {
  let p = String(raw ?? "").trim();
  p = p.replace(/^[a-z][\w+.-]*:\/\/[^/]*/i, "").replace(/^~/, "");
  p = p.split(/[?#]/)[0];
  p = p.replace(/\$\{[^}]*\}|\{[^}]*\}|\\\([^)]*\)|\$[A-Za-z_]\w*|(^|\/):[A-Za-z_]\w*|<[^>]*>|\[[^\]]*\]/g, (m, slash) => `${slash ?? ""}{}`);
  const segs = p
    .split("/")
    .map((s) => s.trim().toLowerCase())
    .filter((s) => s && s !== "." && s !== "..")
    .map((s) => (s.includes("{}") ? "{}" : s));
  while (segs[0] === "{}") segs.shift();
  return segs;
}

const joinRoute = (...parts) =>
  parts
    .map((p) => String(p ?? "").trim())
    .filter(Boolean)
    .join("/")
    .replace(/\/{2,}/g, "/");

/** Annotations, modifiers and attributes in front of a declaration at `at`. */
function prefixOf(code, masked, at) {
  let i = at - 1;
  let depth = 0;
  for (; i >= 0; i--) {
    const x = masked[i];
    if (x === "]" || x === ")") depth++;
    else if (x === "[" || x === "(") depth--;
    else if (depth <= 0 && (x === ";" || x === "{" || x === "}")) break;
  }
  return code.slice(i + 1, at);
}

// C# --------------------------------------------------------------------------------------------

const CS_MEMBER =
  /^((?:\[[\s\S]*?\]\s*)*)((?:(?:public|internal|protected|private|required|virtual|override|new|static|readonly|abstract|sealed|const|unsafe|volatile|extern)\s+)*)([\w.<>[\]?, ]+?)\s+(@?[A-Za-z_]\w*)\s*(\{|=>[\s\S]*|=[\s\S]*)?$/;
const CS_KEYWORDS = /^(class|struct|record|interface|enum|event|delegate|return|using|namespace|operator|implicit|explicit|new|throw|await|var|yield|get|set|init|else|if|case|goto|partial)$/;

function extractCSharp(F, file, code, masked, line) {
  const imports = [...code.matchAll(/^\s*(?:global\s+)?using\s+(?:static\s+)?(?:\w+\s*=\s*)?([\w.]+)\s*;/gm)].map((m) => m[1]);
  const namespaces = [...masked.matchAll(/\bnamespace\s+([\w.]+)/g)].map((m) => ({ at: m.index, name: m[1] }));
  F.imports.set(file, imports.concat(namespaces.map((n) => n.name)));
  const nsAt = (pos) => namespaces.filter((n) => n.at < pos).pop()?.name ?? "";

  for (const m of masked.matchAll(/\b(record\s+struct|record\s+class|class|record|struct|interface|enum)\s+(@?[A-Za-z_]\w*)\s*(<[^>{(;]*>)?\s*(\()?/g)) {
    if (/\bwhere\s+\w+\s*:\s*$/.test(masked.slice(Math.max(0, m.index - 40), m.index))) continue;
    const kind = m[1].startsWith("record") ? "record" : m[1];
    const name = m[2].replace(/^@/, "");
    let pos = m.index + m[0].length;
    const fields = [];
    if (m[4]) {
      const close = matchClose(masked, pos - 1);
      for (const p of splitTop(code.slice(pos, close))) {
        const pm = /^((?:\[[^\]]*\]\s*)*)([\w.<>[\]?, ]+?)\s+(@?\w+)\s*(?:=[\s\S]*)?$/.exec(p);
        if (pm && !ignored(pm[1])) fields.push({ name: pm[3].replace(/^@/, ""), json: jsonName(pm[1]), type: tidy(pm[2]) });
      }
      pos = close + 1;
    }
    const rest = /^([^{;]*)([{;])/.exec(masked.slice(pos, pos + 2000));
    if (!rest) continue;
    const bases = code.slice(pos, pos + rest[1].length).replace(/^\s*:\s*/, "").replace(/\bwhere\b[\s\S]*$/, "").trim();
    const prefix = prefixOf(code, masked, m.index);
    if (rest[2] === "{") {
      const open = pos + rest[1].length;
      const close = matchClose(masked, open);
      const flat = flatten(code, masked, open + 1, close);
      if (kind === "enum") {
        for (const seg of splitTop(flat)) {
          const em = /^((?:\[[\s\S]*?\]\s*)*)(@?[A-Za-z_]\w*)\s*(?:=.*)?$/s.exec(seg);
          if (em) fields.push({ name: em[2].replace(/^@/, ""), json: jsonName(em[1]), type: "enum" });
        }
      } else {
        for (const seg of flat.split(/[;}]/)) {
          const pm = CS_MEMBER.exec(seg.trim());
          if (!pm) continue;
          const [, attrs, mods, type, member, term] = pm;
          if (/\b(static|const)\b/.test(mods) || CS_KEYWORDS.test(type.trim()) || /\b(class|struct|interface|enum|record|event|delegate|operator)\b/.test(type)) continue;
          if (kind !== "interface" && !/\bpublic\b/.test(mods)) continue;
          if (term?.startsWith("=") && !term.startsWith("=>") && !/\bpublic\b/.test(mods)) continue;
          if (ignored(attrs)) continue;
          fields.push({ name: member.replace(/^@/, ""), json: jsonName(attrs), type: tidy(type) });
        }
        if (/Controller(Base)?\b/.test(bases) || /\[\s*(?:ApiController|Route)\b/.test(prefix) || /Controller$/.test(name)) extractController(F, file, code, masked, line, { name, prefix, bases, open, close });
      }
    }
    F.types.push({ name, kind, file, line: line(m.index), namespace: nsAt(m.index), fields, lang: "csharp" });
  }

  // Minimal APIs, with MapGroup prefixes from the same file.
  const groups = new Map();
  for (const m of code.matchAll(/\b(\w+)\s*=\s*[\w.]+\.MapGroup\(\s*"([^"]*)"\s*\)/g)) groups.set(m[1], m[2]);
  for (const m of code.matchAll(/\b(\w+)\s*\.\s*Map(Get|Post|Put|Delete|Patch)\s*\(\s*"([^"]*)"/g)) {
    const open = code.indexOf("(", m.index + m[1].length);
    const close = matchClose(masked, open);
    const call = code.slice(open, close);
    const chain = code.slice(close, close + 400).split(";")[0];
    const request = [];
    const lambda = /\(\s*([^()]*)\)\s*=>/.exec(call);
    for (const p of splitTop(lambda?.[1] ?? "")) {
      const pm = /^((?:\[[^\]]*\]\s*)*)([\w.<>[\]?]+)\s+\w+$/.exec(p);
      if (!pm || /\[From(Route|Query|Header|Services)\]/.test(pm[1])) continue;
      for (const t of typeNames(pm[2])) if (!INJECTED.test(t)) request.push(t);
    }
    const response = [...`${call}${chain}`.matchAll(new RegExp(`(?:Results\\.(?:Ok|Created|Json)|TypedResults\\.(?:Ok|Created|Json)|\\.Produces|Task)\\s*${GEN}`, "g"))].flatMap((x) => typeNames(x[1]));
    const method = m[2].toUpperCase();
    F.endpoints.push({ method, path: joinRoute(groups.get(m[1]) ?? "", m[3]), file, line: line(m.index), handler: `Map${m[2]}`, request: method === "GET" || method === "DELETE" ? [] : request, response: [...new Set(response)] });
  }

  // HttpClient calls.
  const httpRe = new RegExp(`\\.(Get|Post|Put|Patch|Delete)(FromJson|AsJson|String|Stream|ByteArray)?Async\\s*(?:${GEN})?\\s*\\(`, "g");
  for (const m of code.matchAll(httpRe)) {
    const start = m.index + m[0].length;
    const a1 = argAt(code, masked, start);
    if (!a1.text) continue;
    const a2 = a1.last ? null : argAt(code, masked, a1.end + 1);
    const method = m[1].toUpperCase();
    const ahead = code.slice(a1.end, a1.end + 600);
    let response = [];
    let request = [];
    if (m[3] && m[2] === "AsJson") request = typeNames(m[3]);
    else if (m[3]) response = typeNames(m[3]);
    if (!request.length && a2 && method !== "GET") request = typeNames(/^new\s+([\w.<>]+)/.exec(a2.text)?.[1] ?? "");
    if (!response.length) {
      const d = new RegExp(`(?:ReadFromJsonAsync|DeserializeObject|Deserialize(?:Async)?|ReadAsAsync)\\s*${GEN}`).exec(ahead);
      if (d) response = typeNames(d[1]);
    }
    F.calls.push({ method, expr: a1.text, file, line: line(m.index), request, response });
  }
  // new HttpRequestMessage(HttpMethod.Get, url) / new HttpRequestMessage(new HttpMethod("GET"), url)
  for (const m of code.matchAll(/new\s+(?:System\.Net\.Http\.)?HttpRequestMessage\s*\(\s*(?:(?:System\.Net\.Http\.)?HttpMethod\.(Get|Post|Put|Patch|Delete)|new\s+(?:System\.Net\.Http\.)?HttpMethod\s*\(\s*"(\w+)"\s*\))\s*,/g)) {
    const a = argAt(code, masked, m.index + m[0].length);
    if (!a.text) continue;
    const ahead = code.slice(a.end, a.end + 800);
    const d = new RegExp(`(?:ReadFromJsonAsync|DeserializeObject|Deserialize(?:Async)?|ReadAsAsync)\\s*${GEN}`).exec(ahead);
    F.calls.push({ method: (m[1] ?? m[2]).toUpperCase(), expr: a.text, file, line: line(m.index), request: [], response: d ? typeNames(d[1]) : [] });
  }
  if (/\burlBuilder_?\s*\.\s*Append\s*\(/.test(code)) extractNSwag(F, file, code, masked, line);
  if (code.includes('"{+baseurl}')) extractKiota(F, file, code, masked, line);
  // Refit interfaces.
  for (const m of code.matchAll(/(?:\[|,)\s*(Get|Post|Put|Patch|Delete)\s*\(\s*"([^"]*)"\s*\)/g)) {
    const after = code.slice(m.index + m[0].length, m.index + m[0].length + 600);
    const sig = /^(?:\s*\[[\s\S]*?\])*\s*([\w.<>[\]?, ]+?)\s+\w+\s*\(([^)]*)\)/.exec(after);
    if (!sig) continue;
    const body = /\[Body[^\]]*\]\s*([\w.<>[\]?]+)/.exec(sig[2]);
    F.calls.push({ method: m[1].toUpperCase(), expr: JSON.stringify(m[2]), file, line: line(m.index), request: typeNames(body?.[1]), response: typeNames(sig[1]) });
  }

  // Explicit (de)serialization.
  for (const m of code.matchAll(new RegExp(`(?:JsonSerializer\\.Deserialize(?:Async)?|JsonConvert\\.DeserializeObject|ReadFromJsonAsync|GetFromJsonAsync|ReadAsAsync|\\.ToObject|DeserializeAsync)\\s*${GEN}`, "g")))
    addUsage(F, m[1], "deserialize", file, line(m.index), "deserialize");
  for (const m of code.matchAll(new RegExp(`(?:JsonSerializer\\.Serialize(?:Async|ToUtf8Bytes|ToElement)?|JsonConvert\\.SerializeObject|JsonContent\\.Create|(?:Post|Put|Patch)AsJsonAsync)\\s*${GEN}`, "g")))
    addUsage(F, m[1], "serialize", file, line(m.index), "serialize");
  for (const m of code.matchAll(/(?:JsonSerializer\.Serialize\w*|JsonConvert\.SerializeObject|JsonContent\.Create|(?:Post|Put|Patch)AsJsonAsync)\s*\(\s*(?:[^,;()]+,\s*)?new\s+([A-Z][\w.]*)/g))
    addUsage(F, m[1], "serialize", file, line(m.index), "serialize");
  for (const m of code.matchAll(/\[FromBody\]\s*([\w.<>[\]?]+)/g)) addUsage(F, m[1], "deserialize", file, line(m.index), "request body");
}

/** The first positional argument of attribute `name` in `attrs` ([Route("x")], [Authorize, Route(Routes.X)]), or null. */
function attrArg(attrs, name) {
  const m = new RegExp(`(?:\\[|,)\\s*(?:[\\w.]+\\.)?${name}(?:Attribute)?\\s*\\(((?:@?"(?:[^"]|"")*"|[^()"]|\\([^()]*\\))*)\\)`).exec(attrs);
  const first = m ? splitTop(m[1])[0] : undefined;
  return first && !/^\w+\s*=[^=]/.test(first) ? first.replace(/^template\s*:\s*/, "") : null;
}

/** Resolves a controller endpoint's route (constants, [controller], inherited class routes) once the whole project is read. */
function controllerPath(F, r) {
  const resolve = (expr) => (expr == null ? null : resolveUrlExpr(expr, F.consts).replace(/^\{\}\/?/, ""));
  let classExpr = r.classExpr;
  for (let base = r.base, n = 0; classExpr == null && base && n < 5; n++) {
    const parent = F.controllers.get(base);
    classExpr = parent?.route ?? null;
    base = parent?.base;
  }
  const classRoute = resolve(classExpr) ?? "";
  const template = resolve(r.templateExpr);
  const path =
    template !== null && /^~?\//.test(template)
      ? template.replace(/^~/, "")
      : template !== null || classRoute
        ? joinRoute(classRoute, template ?? "")
        : joinRoute(r.ctrl, r.action);
  return path.replace(/\[controller\]/gi, r.ctrl).replace(/\[action\]/gi, r.action).replace(/\[area\]/gi, r.area ?? "{area}");
}

function extractController(F, file, code, masked, line, cls) {
  const classExpr = attrArg(cls.prefix, "Route");
  const base = typeNames(cls.bases.split(",")[0])[0] ?? "";
  F.controllers.set(cls.name, { route: classExpr, base });
  const ctrl = cls.name.replace(/Controller$/, "");
  const area = attrArg(cls.prefix, "Area")?.replace(/^@?"|"$/g, "");
  // [HttpGet], [HttpGet("{id}", Name = "x")], [Authorize, HttpPost(Routes.Create)], [HttpGetAttribute]
  for (const m of masked.slice(cls.open, cls.close).matchAll(/(?:\[|,)\s*(?:[\w.]+\.)?Http(Get|Post|Put|Delete|Patch)(?:Attribute)?\b\s*(\()?/g)) {
    let end = cls.open + m.index + m[0].length;
    let templateExpr = null;
    if (m[2]) {
      const close = matchClose(masked, end - 1);
      const first = splitTop(code.slice(end, close))[0];
      if (first && !/^\w+\s*=[^=]/.test(first)) templateExpr = first.replace(/^template\s*:\s*/, "");
      end = close + 1;
    }
    const at = masked.indexOf("]", end) + 1;
    if (at <= 0) continue;
    const after = code.slice(at, at + 2000);
    const sig = /^((?:\s*\[[\s\S]*?\])*)\s*(?:(?:public|private|protected|internal|virtual|override|async|static|new|sealed)\s+)*([\w.<>[\]?, ]+?)\s+(\w+)\s*\(/.exec(after);
    if (!sig) continue;
    const parenAt = at + sig[0].length - 1;
    const params = code.slice(parenAt + 1, matchClose(masked, parenAt));
    const method = m[1].toUpperCase();
    // Every attribute of the method, before and after the Http one.
    const attrs = prefixOf(code, masked, parenAt);
    if (templateExpr === null) templateExpr = attrArg(attrs, "Route");
    const route = { classExpr, base, templateExpr, ctrl, action: sig[3], area };
    const request = [];
    const parts = splitTop(params);
    const explicit = parts.find((p) => /\[FromBody\]/.test(p));
    for (const p of explicit ? [explicit] : method === "GET" || method === "DELETE" ? [] : parts) {
      const pm = /^((?:\[[^\]]*\]\s*)*)([\w.<>[\]?]+)\s+\w+\s*(?:=.*)?$/.exec(p);
      if (!pm || /\[From(Route|Query|Header|Services|Form)\]/.test(pm[1])) continue;
      const names = typeNames(pm[2]).filter((t) => !INJECTED.test(t));
      if (names.length) {
        request.push(...names);
        break;
      }
    }
    const declared = [...attrs.matchAll(/\b(?:ProducesResponseType|Produces|SwaggerResponse)\s*(?:<([^>]+)>)?\s*\(((?:[^()]|\([^()]*\))*)\)/g)].flatMap((x) => [
      ...typeNames(x[1]),
      ...[...x[2].matchAll(/typeof\s*\(([^)]+)\)/g)].flatMap((t) => typeNames(t[1])),
    ]);
    const response = [...typeNames(sig[2]), ...declared];
    F.endpoints.push({ method, path: "", route, file, line: line(cls.open + m.index), handler: `${cls.name}.${sig[3]}`, request, response: [...new Set(response)] });
  }
}

// Generated C# API clients ---------------------------------------------------------------------

/** Methods with a body in C# source: { name, returns, params, open, close, at }. */
function csMethods(code, masked) {
  const out = [];
  for (const m of masked.matchAll(/\b(?:public|internal|protected|private)\s+(?:(?:virtual|override|async|static|new|sealed|partial)\s+)*([\w.<>[\]?, ]+?)\s+(\w+)\s*\(/g)) {
    const paren = m.index + m[0].length - 1;
    const close = matchClose(masked, paren);
    const next = /^\s*(?:where\b[^{;]*)?([{;]|=>)/.exec(masked.slice(close + 1, close + 300));
    if (next?.[1] !== "{") continue;
    const open = masked.indexOf("{", close + 1);
    out.push({ name: m[2], returns: code.slice(m.index, m.index + m[0].length).match(/\s([\w.<>[\]?, ]+?)\s+\w+\s*\($/)?.[1] ?? m[1], params: code.slice(paren + 1, close), open, close: matchClose(masked, open), at: m.index });
  }
  return out;
}

/** The type of parameter `name` in a C# parameter list. */
function paramType(params, name) {
  for (const p of splitTop(params)) {
    const pm = /^(?:\[[^\]]*\]\s*)*(?:(?:this|ref|in|out|params)\s+)?([\w.<>[\]?, ]+?)\s+@?(\w+)\s*(?:=.*)?$/.exec(p);
    if (pm && pm[2] === name) return pm[1];
  }
  return "";
}

/**
 * NSwag (and similar generators): each operation builds its URL with urlBuilder_.Append(…), sets
 * request_.Method = new HttpMethod("GET") and sends it with SendAsync.
 */
function extractNSwag(F, file, code, masked, line) {
  for (const fn of csMethods(code, masked)) {
    const body = code.slice(fn.open, fn.close);
    const mbody = masked.slice(fn.open, fn.close);
    if (!/\burlBuilder_?\s*\.\s*Append\s*\(/.test(body)) continue;
    const verb = /HttpMethod\s*\(\s*"(\w+)"\s*\)|HttpMethod\.(Get|Post|Put|Patch|Delete)\b/i.exec(body);
    if (!verb) continue;
    const method = (verb[1] ?? verb[2]).toUpperCase();
    let path = "";
    for (const m of mbody.matchAll(/\.\s*Append\s*\(/g)) {
      const stmt = Math.max(mbody.lastIndexOf(";", m.index), mbody.lastIndexOf("{", m.index), mbody.lastIndexOf("}", m.index));
      if (!/\burlBuilder_?\b/.test(mbody.slice(stmt + 1, m.index))) continue;
      const arg = argAt(body, mbody, m.index + m[0].length).text;
      if (/^(['"])\?\1$/.test(arg)) break;
      const lit = /^@?"((?:[^"\\]|\\.)*)"$/.exec(arg);
      if (lit) {
        const q = lit[1].indexOf("?");
        path += q < 0 ? lit[1] : lit[1].slice(0, q);
        if (q >= 0) break;
      } else if (path) path += "{}";
    }
    if (!path.replace(/[{}/]/g, "")) continue;
    const sent = /(?:SerializeToUtf8Bytes|SerializeObject|Serialize|JsonContent\.Create|StringContent)\s*(?:<[^>]*>)?\s*\(\s*(\w+)/.exec(body)?.[1];
    const read = /ReadObjectResponseAsync\s*<([^>]+(?:<[^>]*>)?)>/.exec(body)?.[1];
    F.calls.push({ method, expr: JSON.stringify(path), file, line: line(fn.at), request: sent ? typeNames(paramType(fn.params, sent)) : [], response: typeNames(read ?? fn.returns) });
  }
}

/** Kiota request builders: base(requestAdapter, "{+baseurl}/api/orders{?page}", …) with GetAsync/PostAsync/… methods. */
function extractKiota(F, file, code, masked, line) {
  const template = /"\{\+baseurl\}([^"]*)"/.exec(code)?.[1];
  if (template === undefined) return;
  const path = template.replace(/\{[?&][^}]*\}/g, "");
  if (!path.replace(/[{}/]/g, "")) return;
  for (const fn of csMethods(code, masked)) {
    const verb = /^(Get|Post|Put|Patch|Delete)Async$/.exec(fn.name);
    if (!verb || !/\bSend\w*Async\b|\bTo\w+RequestInformation\b/.test(code.slice(fn.open, fn.close))) continue;
    F.calls.push({ method: verb[1].toUpperCase(), expr: JSON.stringify(path), file, line: line(fn.at), request: typeNames(paramType(fn.params, "body")), response: typeNames(fn.returns) });
  }
}

// OpenAPI / Swagger documents ------------------------------------------------------------------

/** OpenAPI documents a client is generated from (swagger.json, OpenAPIs/*.yaml) and NSwag configs that embed one. */
const SPEC_FILE = /(?:swagger|openapi|api[-_.]?spec)[^/]*\.(?:json|ya?ml)$|(?:^|\/)openapis\/[^/]+\.(?:json|ya?ml)$|\.nswag$|(?:^|\/)nswag\.json$/i;

/** A small block-style YAML reader: enough for OpenAPI documents (maps, lists, scalars, simple flow values). */
export function parseYaml(text) {
  const rows = [];
  for (const raw of String(text).split(/\r?\n/)) {
    if (/^\s*(#|$)/.test(raw) || /^(---|\.\.\.)\s*$/.test(raw)) continue;
    rows.push({ indent: raw.length - raw.trimStart().length, text: raw.trim() });
  }
  const unquote = (s) => (/^(['"]).*\1$/s.test(s) ? s.slice(1, -1) : s);
  const keyOf = (s) => {
    const m = /^("[^"]*"|'[^']*'|[^:#'"][^:]*?|\?)\s*:(?:\s+(.*)|$)/.exec(s);
    return m ? [unquote(m[1].trim()), (m[2] ?? "").trim()] : null;
  };
  const scalar = (raw) => {
    const s = raw.replace(/\s+#.*$/, "").trim();
    if (/^(['"]).*\1$/s.test(s)) return s.slice(1, -1);
    if (/^\[.*\]$/s.test(s)) return splitTop(s.slice(1, -1)).map(scalar);
    if (/^\{.*\}$/s.test(s)) return Object.fromEntries(splitTop(s.slice(1, -1)).map((kv) => keyOf(kv)).filter(Boolean).map(([k, v]) => [k, scalar(v)]));
    if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s);
    if (s === "true" || s === "false") return s === "true";
    if (s === "null" || s === "~") return null;
    return s;
  };
  const isItem = (r) => r.text === "-" || r.text.startsWith("- ");
  let i = 0;
  function block(indent) {
    if (isItem(rows[i])) {
      const out = [];
      while (i < rows.length && rows[i].indent >= indent) {
        if (rows[i].indent > indent || !isItem(rows[i])) {
          i++;
          continue;
        }
        const rest = rows[i].text.slice(1).trim();
        if (!rest) {
          i++;
          out.push(i < rows.length && rows[i].indent > indent ? block(rows[i].indent) : null);
        } else if (keyOf(rest)) {
          // "- name: x" starts a map whose other keys sit under "name".
          rows[i] = { indent: indent + 2, text: rest };
          out.push(block(indent + 2));
        } else {
          i++;
          out.push(scalar(rest));
        }
      }
      return out;
    }
    const out = {};
    while (i < rows.length && rows[i].indent >= indent) {
      const kv = rows[i].indent === indent && keyOf(rows[i].text);
      i++;
      if (!kv) continue;
      const [key, rest] = kv;
      if (!rest) {
        const nested = i < rows.length && (rows[i].indent > indent || (rows[i].indent === indent && isItem(rows[i])));
        out[key] = nested ? block(rows[i].indent) : null;
      } else if (/^[|>][+-]?\d*$/.test(rest)) {
        const lines = [];
        while (i < rows.length && rows[i].indent > indent) lines.push(rows[i++].text);
        out[key] = lines.join("\n");
      } else out[key] = scalar(rest);
    }
    return out;
  }
  return rows.length ? block(rows[0].indent) : null;
}

function parseSpec(file, text) {
  const body = String(text).replace(/^﻿/, "");
  if (/\.nswag$|nswag\.json$/i.test(file)) {
    let config;
    try {
      config = JSON.parse(body);
    } catch {
      return null;
    }
    const inner = config?.documentGenerator?.fromDocument?.json;
    return typeof inner === "string" && inner.trim() ? parseSpec("embedded", inner) : null;
  }
  if (/^\s*[{[]/.test(body)) {
    try {
      return JSON.parse(body);
    } catch {
      return null;
    }
  }
  return parseYaml(body);
}

/** Operations and schemas of an OpenAPI 3 or Swagger 2 document. */
export function extractSpec(F, file, text) {
  const spec = parseSpec(file, text);
  if (!spec?.paths || typeof spec.paths !== "object" || !(spec.openapi || spec.swagger)) return;
  const line = lineIndex(text);
  const at = (needle, from = 0) => {
    const i = text.indexOf(needle, from);
    return i < 0 ? -1 : i;
  };
  const refs = (schema) => {
    if (!schema || typeof schema !== "object") return [];
    if (typeof schema.$ref === "string") return [schema.$ref.split("/").pop()];
    if (schema.items) return refs(schema.items);
    for (const k of ["allOf", "oneOf", "anyOf"]) if (Array.isArray(schema[k])) return schema[k].flatMap(refs);
    return [];
  };
  const bodyOf = (content) => (content && typeof content === "object" ? refs((content["application/json"] ?? Object.values(content)[0])?.schema) : []);
  const serverPath = typeof spec.basePath === "string" ? spec.basePath : (/^(?:[a-z]+:\/\/[^/]*)?(\/[^?#]*)/i.exec(spec.servers?.[0]?.url ?? "")?.[1] ?? "");
  const ops = [];
  for (const [path, item] of Object.entries(spec.paths)) {
    const pathAt = Math.max(at(`"${path}"`), at(`${path}:`), 0);
    for (const verb of ["get", "post", "put", "patch", "delete"]) {
      const op = item?.[verb];
      if (!op || typeof op !== "object") continue;
      let request = bodyOf(op.requestBody?.content);
      if (!request.length && Array.isArray(op.parameters)) request = refs(op.parameters.find((p) => p?.in === "body")?.schema);
      const ok = Object.keys(op.responses ?? {}).find((k) => /^2/.test(k));
      const res = ok ? op.responses[ok] : null;
      const response = res ? (res.content ? bodyOf(res.content) : refs(res.schema)) : [];
      const verbAt = Math.max(at(`"${verb}"`, pathAt), at(`${verb}:`, pathAt));
      ops.push({ method: verb.toUpperCase(), path: joinRoute(serverPath, path), file, line: line(verbAt >= 0 ? verbAt : pathAt), request, response, handler: op.operationId ?? "" });
    }
  }
  const typeOf = (s) => {
    if (!s || typeof s !== "object") return "";
    if (typeof s.$ref === "string") return s.$ref.split("/").pop();
    if (s.type === "array") return `${typeOf(s.items) || "object"}[]`;
    if (Array.isArray(s.allOf) && s.allOf.length === 1) return typeOf(s.allOf[0]);
    return String(s.format === "date-time" || s.format === "uuid" ? "string" : (Array.isArray(s.type) ? s.type.find((t) => t !== "null") : s.type) ?? "");
  };
  const types = [];
  for (const [name, s] of Object.entries(spec.components?.schemas ?? spec.definitions ?? {})) {
    if (!s || typeof s !== "object") continue;
    const nameAt = Math.max(at(`"${name}"`), at(`${name}:`), 0);
    const fields = Array.isArray(s.enum)
      ? s.enum.map((v) => ({ name: String(v), json: "", type: "enum" }))
      : Object.entries(s.properties ?? Object.assign({}, ...(s.allOf ?? []).map((x) => x?.properties ?? {}))).map(([k, v]) => ({ name: k, json: k, type: typeOf(v) }));
    types.push({ name, kind: Array.isArray(s.enum) ? "enum" : "schema", file, line: line(nameAt), namespace: "", fields, lang: "openapi" });
  }
  F.files++;
  F.langs.openapi = (F.langs.openapi ?? 0) + 1;
  F.specs.push({ file, ops, types });
}

/**
 * A project's OpenAPI documents describe the API it calls (a client generated from them), unless
 * most of a document's operations are the project's own endpoints (the API's exported spec).
 */
function specClients(F) {
  const have = new Set(F.calls.map((c) => `${c.method} ${c.segs.join("/")}`));
  for (const spec of F.specs) {
    const own = spec.ops.filter((o) => F.endpoints.some((e) => sameMethod(e.method, o.method) && pathScore(e.segs, normPath(o.path)) > 0)).length;
    if (spec.ops.length && own >= spec.ops.length / 2) continue;
    for (const o of spec.ops) {
      const segs = normPath(o.path);
      const key = `${o.method} ${segs.join("/")}`;
      if (have.has(key) || !segs.some((x) => x !== "{}")) continue;
      have.add(key);
      F.calls.push({ method: o.method, path: o.path, segs, file: o.file, line: o.line, request: o.request, response: o.response });
    }
    for (const t of spec.types) if (!F.types.some((x) => x.name === t.name)) F.types.push(t);
  }
}

// TypeScript / JavaScript -----------------------------------------------------------------------

const SERVER_RECEIVER = /^(app|router|server|fastify|routes?|apiRouter|hono|express)$/;
const CLIENT_RECEIVER = /^(axios|http|https|client|apiClient|httpClient|instance|ky|request|\$http|dio|_dio|_client|_http)$/;

function extractTs(F, file, code, masked, line) {
  F.imports.set(file, [...code.matchAll(/\bfrom\s+["']([^"']+)["']|\brequire\(\s*["']([^"']+)["']\s*\)|\bimport\s+["']([^"']+)["']/g)].map((m) => m[1] ?? m[2] ?? m[3]));
  const declare = (name, kind, open, at) => {
    const close = matchClose(masked, open);
    const flat = flatten(code, masked, open + 1, close);
    const fields = [];
    for (const seg of splitTop(flat, ";,\n")) {
      const s = seg.replace(/^(?:@\w+(?:\([^)]*\))?\s*)+/, "");
      if (/^(private|protected|static|#)/.test(s)) continue;
      const fm = /^(?:(?:readonly|public|declare|override)\s+)*["']?([A-Za-z_$][\w$-]*)["']?\s*([?!])?\s*:\s*([^=]+?)\s*(?:=[\s\S]*)?$/.exec(s);
      if (fm) fields.push({ name: fm[1], json: "", type: tidy(fm[3]), optional: fm[2] === "?" });
    }
    if (fields.length) F.types.push({ name, kind, file, line: line(at), namespace: "", fields, lang: "ts" });
  };
  for (const m of masked.matchAll(/\b(interface|class)\s+([A-Za-z_$][\w$]*)\s*(<[^>{]*>)?[^{;=]*\{/g)) declare(m[2], m[1], m.index + m[0].length - 1, m.index);
  for (const m of masked.matchAll(/\btype\s+([A-Za-z_$][\w$]*)\s*(<[^>=]*>)?\s*=\s*\{/g)) declare(m[1], "type", m.index + m[0].length - 1, m.index);

  // NestJS controllers.
  const controllers = [...code.matchAll(/@Controller\(\s*(?:["'`]([^"'`]*)["'`])?/g)].map((m) => ({ at: m.index, prefix: m[1] ?? "" }));
  for (const m of code.matchAll(/@(Get|Post|Put|Patch|Delete)\(\s*(?:["'`]([^"'`]*)["'`])?\s*\)/g)) {
    const ctrl = controllers.filter((c) => c.at < m.index).pop();
    if (!ctrl) continue;
    const after = code.slice(m.index, m.index + 800);
    const body = /@Body\([^)]*\)\s*\w+\s*:\s*([\w$.<>[\]]+)/.exec(after);
    const ret = /\)\s*:\s*(?:Promise<)?([\w$.<>[\]]+)>?\s*\{/.exec(after);
    F.endpoints.push({ method: m[1].toUpperCase(), path: joinRoute(ctrl.prefix, m[2] ?? ""), file, line: line(m.index), handler: "", request: typeNames(body?.[1]), response: typeNames(ret?.[1]) });
  }

  // Next.js route handlers, from the file's path.
  const next = /(?:^|\/)app\/(api\/.+)\/route\.[jt]sx?$/.exec(file) ?? /(?:^|\/)pages\/(api\/.+?)(?:\/index)?\.[jt]sx?$/.exec(file);
  if (next) {
    const methods = [...code.matchAll(/export\s+(?:async\s+)?(?:function|const)\s+(GET|POST|PUT|PATCH|DELETE)\b/g)].map((m) => m[1]);
    for (const method of methods.length ? methods : ["*"]) F.endpoints.push({ method, path: next[1], file, line: 1, handler: "route handler", request: [], response: [] });
  }

  // fetch()
  for (const m of code.matchAll(new RegExp(`\\b(?:fetch|\\$fetch|ofetch|useFetch|ky)\\s*(?:${GEN})?\\s*\\(`, "g"))) {
    const a1 = argAt(code, masked, m.index + m[0].length);
    if (!a1.text) continue;
    const a2 = a1.last ? null : argAt(code, masked, a1.end + 1);
    const method = (/method\s*:\s*["'`](\w+)["'`]/.exec(a2?.text ?? "")?.[1] ?? "GET").toUpperCase();
    const ahead = code.slice(a1.end, a1.end + 400);
    const cast = /(?:\.json\(\)|\.json<([\w$.<>[\]]+)>\(\))\s*\)?\s*(?:as\s+(?:Promise<)?([\w$.<>[\]]+))?/.exec(ahead);
    F.calls.push({ method, expr: a1.text, file, line: line(m.index), request: [], response: typeNames(m[1] ?? cast?.[1] ?? cast?.[2]) });
  }
  extractReceiverCalls(F, file, code, masked, line);
  for (const m of code.matchAll(/JSON\.parse\([^;]*?\)\s*as\s+([\w$.<>[\]]+)/g)) addUsage(F, m[1], "deserialize", file, line(m.index), "JSON.parse");
  for (const m of code.matchAll(/\.json\(\)\)?\s*as\s+(?:Promise<)?([\w$.<>[\]]+)/g)) addUsage(F, m[1], "deserialize", file, line(m.index), "response.json()");
}

/** receiver.get("/x") style calls (axios, Angular, ky, dio, Ktor client) and app.get("/x", handler) routes. */
function extractReceiverCalls(F, file, code, masked, line) {
  const re = new RegExp(`\\b([A-Za-z_$][\\w$]*)\\s*\\.\\s*(get|post|put|patch|delete)\\s*(?:${GEN})?\\s*\\(`, "g");
  for (const m of code.matchAll(re)) {
    const a1 = argAt(code, masked, m.index + m[0].length);
    if (!a1.text || !/["'`]|^[A-Z_]{2,}|\w+\.\w+|Uri\.parse/.test(a1.text)) continue;
    const a2 = a1.last ? null : argAt(code, masked, a1.end + 1);
    const receiver = m[1];
    const method = m[2].toUpperCase();
    const server = SERVER_RECEIVER.test(receiver) || (!!a2 && /=>|^\s*(async\s+)?function\b/.test(a2.text) && !CLIENT_RECEIVER.test(receiver));
    if (server) {
      F.endpoints.push({ method, path: resolveUrlExpr(a1.text, new Map()), file, line: line(m.index), handler: receiver, request: [], response: [] });
      continue;
    }
    const ahead = code.slice(a1.end, a1.end + 500);
    const response = typeNames(m[3]?.split(",")[0] ?? /([A-Z]\w*)\.fromJson\(|\.body<([\w.<>]+)>\(\)/.exec(ahead)?.slice(1).find(Boolean) ?? "");
    F.calls.push({ method, expr: a1.text, file, line: line(m.index), request: [], response });
  }
}

// Kotlin / Java ---------------------------------------------------------------------------------

const JVM_SKIP = /(Service|Repository|Controller|Activity|Fragment|ViewModel|Adapter|Manager|Helper|Module|Component|Application|Test|Factory|Builder|Impl)$/;

function extractJvm(F, file, code, masked, line, lang) {
  F.imports.set(file, [...code.matchAll(/^\s*import\s+(?:static\s+)?([\w.]+)/gm)].map((m) => m[1]));
  for (const m of masked.matchAll(/\b((?:(?:data|value|open|sealed|abstract|public|private|internal|final|enum)\s+)*)(class|record|interface)\s+(\w+)\s*(<[^>]*>)?/g)) {
    const name = m[3];
    if (JVM_SKIP.test(name) || m[2] === "interface") continue;
    const prefix = prefixOf(code, masked, m.index);
    const fields = [];
    let pos = m.index + m[0].length;
    const ctor = /^\s*(?:@[\w.]+(?:\([^)]*\))?\s*)*(?:(?:private|internal|public|protected)\s+)?(?:constructor\s*)?\(/.exec(masked.slice(pos, pos + 300));
    if (ctor) {
      const open = pos + ctor[0].length - 1;
      const close = matchClose(masked, open);
      for (const p of splitTop(code.slice(open + 1, close))) {
        const km = /^((?:@[\w.:]+(?:\([^)]*\))?\s*)*)(?:(?:override|private|public|internal|protected|open|final)\s+)*(?:val|var)\s+(\w+)\s*:\s*([^=]+?)\s*(?:=[\s\S]*)?$/.exec(p);
        const jm = lang === "java" && /^((?:@[\w.]+(?:\([^)]*\))?\s*)*)(?:final\s+)?([\w.<>[\], ?]+?)\s+(\w+)$/.exec(p);
        if (km && !/^(?:@[\w.:]+(?:\([^)]*\))?\s*)*private\b/.test(p)) fields.push({ name: km[2], json: jsonName(km[1]), type: tidy(km[3]) });
        else if (jm && m[2] === "record") fields.push({ name: jm[3], json: jsonName(jm[1]), type: tidy(jm[2]) });
      }
      pos = close + 1;
    }
    const rest = /^([^{;\n]*)\{/.exec(masked.slice(pos, pos + 1000));
    if (rest) {
      const open = pos + rest[0].length - 1;
      const close = matchClose(masked, open);
      const flat = flatten(code, masked, open + 1, close);
      if (/\benum\b/.test(m[1])) {
        const head = flat.split(";")[0];
        for (const seg of splitTop(head)) {
          const em = /^((?:@[\w.]+(?:\([^)]*\))?\s*)*)([A-Za-z_]\w*)/.exec(seg);
          if (em) fields.push({ name: em[2], json: jsonName(em[1]), type: "enum" });
        }
      } else {
        for (const seg of flat.split(lang === "java" ? /[;}]/ : /[\n;}]/)) {
          const s = seg.trim();
          if (/\b(static|const|companion|fun|class|object)\b/.test(s)) continue;
          if (lang === "kotlin") {
            const km = /^((?:@[\w.:]+(?:\([^)]*\))?\s*)*)(?:(?:override|public|internal|open|lateinit)\s+)*(?:val|var)\s+(\w+)\s*:\s*([^={]+?)\s*(=[\s\S]*)?$/.exec(s);
            if (km && !ignored(km[1])) fields.push({ name: km[2], json: jsonName(km[1]), type: tidy(km[3]) });
          } else {
            const jm = /^((?:@[\w.]+(?:\([^)]*\))?\s*)*)(?:private|public|protected)\s+(?:(?:final|transient|volatile)\s+)*([\w.<>[\], ?]+?)\s+(\w+)\s*(?:=[\s\S]*)?$/.exec(s);
            if (jm && !ignored(jm[1]) && !/\btransient\b/.test(s)) fields.push({ name: jm[3], json: jsonName(jm[1]), type: tidy(jm[2]) });
          }
        }
      }
    }
    const modelish = /\bdata\b|\benum\b/.test(m[1]) || m[2] === "record" || /@(Serializable|JsonClass|Data|Value|Parcelize|JsonInclude)\b/.test(prefix) || fields.some((f) => f.json);
    if (fields.length && (modelish || lang === "java")) F.types.push({ name, kind: /\benum\b/.test(m[1]) ? "enum" : m[2], file, line: line(m.index), namespace: /^\s*package\s+([\w.]+)/m.exec(code)?.[1] ?? "", fields, lang });
  }

  // Retrofit
  for (const m of code.matchAll(/@(GET|POST|PUT|PATCH|DELETE)\(\s*(?:value\s*=\s*)?"([^"]*)"\s*\)/g)) {
    const after = code.slice(m.index + m[0].length, m.index + m[0].length + 600);
    const kt = /fun\s+\w+\s*\(([^)]*)\)\s*(?::\s*([^\n{=]+))?/.exec(after);
    const jv = /([\w.<>[\], ?]+?)\s+\w+\s*\(([^)]*)\)\s*;/.exec(after);
    const params = kt?.[1] ?? jv?.[2] ?? "";
    const body = /@Body\s+\w+\s*:\s*([\w.<>?]+)/.exec(params) ?? /@Body\s+([\w.<>]+)\s+\w+/.exec(params);
    F.calls.push({ method: m[1], expr: JSON.stringify(m[2]), file, line: line(m.index), request: typeNames(body?.[1]), response: typeNames(kt?.[2] ?? jv?.[1]) });
  }
  // Spring
  const classAt = masked.search(/\bclass\s+\w+/);
  const classPrefix = classAt > 0 ? code.slice(0, classAt) : "";
  const base = /@RequestMapping\(\s*(?:value\s*=\s*|path\s*=\s*)?\[?\s*"([^"]*)"/.exec(classPrefix)?.[1] ?? "";
  for (const m of code.matchAll(/@(Get|Post|Put|Patch|Delete|Request)Mapping(?:\(\s*(?:value\s*=\s*|path\s*=\s*)?\[?\s*"([^"]*)"[^)]*\))?/g)) {
    if (m.index < classAt) continue;
    const after = code.slice(m.index + m[0].length, m.index + m[0].length + 800);
    const method = m[1] === "Request" ? /RequestMethod\.(\w+)/.exec(m[0])?.[1] ?? "*" : m[1].toUpperCase();
    const body = /@RequestBody\s+(?:\w+\s*:\s*)?([\w.<>?]+)/.exec(after);
    const ret = /fun\s+\w+\s*\([^)]*\)\s*:\s*([^\n{=]+)/.exec(after) ?? /public\s+([\w.<>[\], ?]+?)\s+\w+\s*\(/.exec(after);
    F.endpoints.push({ method, path: joinRoute(base, m[2] ?? ""), file, line: line(m.index), handler: "", request: typeNames(body?.[1]), response: typeNames(ret?.[1]) });
  }
  // Ktor server routing
  if (lang === "kotlin") {
    for (const m of code.matchAll(/(^|[^.\w])(get|post|put|patch|delete)\s*\(\s*"([^"]*)"\s*\)\s*\{/g)) {
      const body = code.slice(m.index, m.index + 600);
      F.endpoints.push({ method: m[2].toUpperCase(), path: m[3], file, line: line(m.index + m[1].length), handler: "ktor", request: typeNames(/call\.receive<([\w.<>]+)>/.exec(body)?.[1]), response: [] });
    }
  }
  extractReceiverCalls(F, file, code, masked, line);
  for (const m of code.matchAll(/(?:fromJson|readValue|convertValue|adapter)\s*\(\s*(?:[^,()]+,\s*)?([A-Z]\w*)(?:::class\.java|\.class|::class)/g)) addUsage(F, m[1], "deserialize", file, line(m.index), "deserialize");
  for (const m of code.matchAll(new RegExp(`decodeFromString\\s*${GEN}|decodeFromString\\(\\s*([A-Z]\\w*)\\.serializer\\(\\)|\\.body\\s*${GEN}\\s*\\(\\)`, "g")))
    addUsage(F, m[1] ?? m[2] ?? m[3], "deserialize", file, line(m.index), "deserialize");
  for (const m of code.matchAll(new RegExp(`encodeToString\\s*${GEN}|encodeToString\\(\\s*([A-Z]\\w*)\\.serializer\\(\\)`, "g"))) addUsage(F, m[1] ?? m[2], "serialize", file, line(m.index), "serialize");
}

// Swift -----------------------------------------------------------------------------------------

function extractSwift(F, file, code, masked, line) {
  F.imports.set(file, [...code.matchAll(/^\s*import\s+(\w+)/gm)].map((m) => m[1]));
  for (const m of masked.matchAll(/\b(struct|class|enum)\s+(\w+)\s*(?:<[^>]*>)?\s*(?::\s*([^{]+))?\{/g)) {
    const conforms = m[3] ?? "";
    if (!/\b(Codable|Decodable|Encodable)\b/.test(conforms)) continue;
    const open = m.index + m[0].length - 1;
    const close = matchClose(masked, open);
    const body = code.slice(open + 1, close);
    const flat = flatten(code, masked, open + 1, close);
    const keys = new Map();
    const ck = /enum\s+CodingKeys[^{]*\{([^}]*)\}/.exec(body);
    for (const c of ck ? ck[1].matchAll(/case\s+([^\n]+)/g) : []) for (const item of splitTop(c[1])) {
      const km = /^(\w+)\s*(?:=\s*"([^"]*)")?/.exec(item);
      if (km) keys.set(km[1], km[2] ?? km[1]);
    }
    const fields = [];
    if (m[1] === "enum") {
      for (const c of flat.matchAll(/case\s+([^\n]+)/g)) for (const item of splitTop(c[1])) {
        const em = /^(\w+)\s*(?:=\s*"([^"]*)")?/.exec(item);
        if (em) fields.push({ name: em[1], json: em[2] ?? "", type: "enum" });
      }
    } else {
      for (const seg of flat.split(/[\n;]/)) {
        const fm = /^\s*(?:@\w+(?:\([^)]*\))?\s*)*(?:(?:public|internal|open|final|private\(set\))\s+)*(?:let|var)\s+(\w+)\s*:\s*([^={]+?)\s*(=[^{]*)?(\{\})?\s*$/.exec(seg);
        if (!fm || fm[4] || /\b(static|private|fileprivate|lazy)\b/.test(seg)) continue;
        if (keys.size && !keys.has(fm[1])) continue;
        fields.push({ name: fm[1], json: keys.get(fm[1]) !== fm[1] ? keys.get(fm[1]) ?? "" : "", type: tidy(fm[2]) });
      }
    }
    if (fields.length) F.types.push({ name: m[2], kind: m[1], file, line: line(m.index), namespace: "", fields, lang: "swift" });
  }
  for (const m of code.matchAll(/\.decode\(\s*\[?([A-Z]\w*)\]?\.self|responseDecodable\(\s*of:\s*\[?([A-Z]\w*)\]?\.self/g)) addUsage(F, m[1] ?? m[2], "deserialize", file, line(m.index), "decode");
  for (const m of code.matchAll(/\bAF\s*\.\s*request\s*\(/g)) {
    const a1 = argAt(code, masked, m.index + m[0].length);
    const tail = code.slice(a1.end, a1.end + 300);
    F.calls.push({ method: (/method:\s*\.(\w+)/.exec(tail)?.[1] ?? "get").toUpperCase(), expr: a1.text, file, line: line(m.index), request: [], response: typeNames(/responseDecodable\(\s*of:\s*\[?([A-Z]\w*)/.exec(tail)?.[1]) });
  }
}

// Dart ------------------------------------------------------------------------------------------

function extractDart(F, file, code, masked, line) {
  F.imports.set(file, [...code.matchAll(/^\s*import\s+['"]package:([\w]+)\//gm)].map((m) => m[1]));
  for (const m of masked.matchAll(/\b(class|enum)\s+(\w+)\s*(?:<[^>]*>)?[^{;]*\{/g)) {
    const open = m.index + m[0].length - 1;
    const close = matchClose(masked, open);
    const body = code.slice(open + 1, close);
    const prefix = prefixOf(code, masked, m.index);
    if (m[1] === "class" && !/fromJson|toJson/.test(body) && !/@(JsonSerializable|freezed|Freezed)\b/.test(prefix)) continue;
    const fields = [];
    const flat = flatten(code, masked, open + 1, close);
    if (m[1] === "enum") {
      for (const seg of splitTop(flat.split(";")[0])) {
        const em = /^((?:@\w+(?:\([^)]*\))?\s*)*)(\w+)/.exec(seg);
        if (em) fields.push({ name: em[2], json: /JsonValue\(\s*['"]([^'"]*)['"]/.exec(em[1])?.[1] ?? "", type: "enum" });
      }
    } else {
      for (const seg of flat.split(/[;}]/)) {
        const s = seg.trim();
        if (/\b(static|const|factory|get|set|class)\b|\(/.test(s.replace(/@\w+\([^)]*\)/g, ""))) continue;
        const fm = /^((?:@\w+(?:\([^)]*\))?\s*)*)(?:(?:final|late)\s+)*([\w<>?, ]+?)\s+(\w+)\s*(?:=[\s\S]*)?$/.exec(s);
        if (fm && !ignored(fm[1]) && !fm[3].startsWith("_")) fields.push({ name: fm[3], json: jsonName(fm[1]), type: tidy(fm[2]) });
      }
      const freezed = /factory\s+\w+(?:\.\w+)?\s*\(\s*\{([^}]*)\}/.exec(body);
      for (const p of freezed ? splitTop(freezed[1]) : []) {
        const fm = /^((?:@\w+(?:\([^)]*\))?\s*)*)(?:required\s+)?([\w<>?, ]+?)\s+(\w+)\s*(?:=.*)?$/.exec(p);
        if (fm && !fields.some((f) => f.name === fm[3])) fields.push({ name: fm[3], json: jsonName(fm[1]), type: tidy(fm[2]) });
      }
    }
    if (fields.length) F.types.push({ name: m[2], kind: m[1], file, line: line(m.index), namespace: "", fields, lang: "dart" });
  }
  extractReceiverCalls(F, file, code, masked, line);
  for (const m of code.matchAll(/\b([A-Z]\w*)\.fromJson\(/g)) addUsage(F, m[1], "deserialize", file, line(m.index), "fromJson");
}

// Go, Rust and Zig shared helpers --------------------------------------------------------------

/** Functions of a file: [{ name, params, returns, open, close, at }] for every `re` match ending at the "(" of the parameters. */
function fnsOf(code, masked, re) {
  const out = [];
  for (const m of masked.matchAll(re)) {
    const paren = m.index + m[0].length - 1;
    const close = matchClose(masked, paren);
    const open = masked.indexOf("{", close);
    const semi = masked.indexOf(";", close);
    if (open < 0 || (semi >= 0 && semi < open)) continue;
    out.push({ name: m[1], params: code.slice(paren + 1, close), returns: code.slice(close + 1, open), open, close: matchClose(masked, open), at: m.index });
  }
  return out;
}

const fnAt = (fns, pos) => fns.filter((f) => f.open < pos && pos < f.close).pop();
const lastName = (expr) => /([A-Za-z_]\w*)\s*$/.exec(String(expr ?? ""))?.[1] ?? "";

// Go --------------------------------------------------------------------------------------------

/** What a Go function decodes from and encodes into JSON (types resolved through its variables). */
function goPayloads(code, masked, fn) {
  if (!fn) return { decoded: [], encoded: [] };
  const body = code.slice(fn.open, fn.close);
  const mbody = masked.slice(fn.open, fn.close);
  const vars = new Map();
  for (const p of splitTop(fn.params)) {
    const pm = /^(\w+)\s+\*?([\w.[\]]+)$/.exec(p);
    if (pm) vars.set(pm[1], pm[2]);
  }
  for (const m of body.matchAll(/\bvar\s+(\w+)\s+\*?((?:\[\])?\*?[\w.]+)/g)) vars.set(m[1], m[2]);
  for (const m of body.matchAll(/\b(\w+)\s*:?=\s*&?((?:\[\])?\*?[\w.]+)\s*\{/g)) vars.set(m[1], m[2]);
  for (const m of body.matchAll(/\b(\w+)\s*:?=\s*new\(\s*([\w.]+)\s*\)/g)) vars.set(m[1], m[2]);
  const typeOf = (expr) => {
    const e = String(expr ?? "").trim().replace(/^&/, "");
    return /^((?:\[\])?\*?[\w.]+)\s*\{/.exec(e)?.[1] ?? vars.get(e) ?? "";
  };
  const decoded = [];
  const encoded = [];
  for (const m of mbody.matchAll(/\.(?:ShouldBindJSON|BindJSON|ShouldBind|Bind|BodyParser|Decode)\s*\(/g)) decoded.push(...typeNames(typeOf(argAt(body, mbody, m.index + m[0].length).text)));
  for (const m of mbody.matchAll(/\bjson\.Unmarshal\s*\(/g)) {
    const a = argAt(body, mbody, m.index + m[0].length);
    if (!a.last) decoded.push(...typeNames(typeOf(argAt(body, mbody, a.end + 1).text)));
  }
  for (const m of mbody.matchAll(/\.(?:JSON|IndentedJSON|PureJSON|Encode)\s*\(|\bjson\.Marshal(?:Indent)?\s*\(/g)) {
    const a = argAt(body, mbody, m.index + m[0].length);
    // c.JSON(http.StatusOK, x) (gin, echo) or c.JSON(x) (fiber).
    const value = !a.last && /\.(?:JSON|IndentedJSON|PureJSON)/.test(m[0]) ? argAt(body, mbody, a.end + 1).text : a.text;
    encoded.push(...typeNames(typeOf(value)));
  }
  for (const m of body.matchAll(/\.SetResult\(\s*&?([\w.]+)\s*\{?/g)) decoded.push(...typeNames(typeOf(m[1]) || m[1]));
  for (const m of body.matchAll(/\.SetBody\(\s*([^)]+)\)/g)) encoded.push(...typeNames(typeOf(m[1])));
  return { decoded: [...new Set(decoded)], encoded: [...new Set(encoded)] };
}

const GO_METHOD = { MethodGet: "GET", MethodPost: "POST", MethodPut: "PUT", MethodPatch: "PATCH", MethodDelete: "DELETE" };

function extractGo(F, file, code, masked, line) {
  const imports = [];
  for (const m of code.matchAll(/\bimport\s*\(([^)]*)\)|\bimport\s+(?:[\w.]+\s+)?"([^"]+)"/g)) imports.push(...(m[2] ? [m[2]] : [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1])));
  F.imports.set(file, imports);

  for (const m of masked.matchAll(/\btype\s+(\w+)\s+struct\s*\{/g)) {
    const open = m.index + m[0].length - 1;
    const fields = [];
    for (const row of flatten(code, masked, open + 1, matchClose(masked, open)).split(/[\n;]/)) {
      const fm = /^\s*(\w+(?:\s*,\s*\w+)*)\s+([^`\s][^`]*?)\s*(`[^`]*`)?\s*$/.exec(row);
      if (!fm) continue;
      const tag = /json:"([^"]*)"/.exec(fm[3] ?? "")?.[1];
      if (tag?.split(",")[0] === "-") continue;
      for (const name of fm[1].split(/\s*,\s*/)) if (/^[A-Z]/.test(name) || tag) fields.push({ name, json: tag?.split(",")[0] ?? "", type: tidy(fm[2]) });
    }
    if (fields.length) F.types.push({ name: m[1], kind: "struct", file, line: line(m.index), namespace: "", fields, lang: "go" });
  }

  const fns = fnsOf(code, masked, /\bfunc\s+(?:\([^)]*\)\s*)?(\w+)\s*(?:\[[^\]]*\])?\s*\(/g);
  const byName = new Map(fns.map((f) => [f.name, f]));
  const handlerFn = (expr, pos) => {
    const inline = /^func\s*\(/.test(expr.trim()) ? fnsOf(code, masked, /\bfunc\s*()\(/g).find((f) => f.at >= pos && f.at < pos + 400) : null;
    return inline ?? byName.get(lastName(expr));
  };
  for (const fn of fns) {
    const p = goPayloads(code, masked, fn);
    addUsage(F, p.decoded, "deserialize", file, line(fn.at), "json decode");
    addUsage(F, p.encoded, "serialize", file, line(fn.at), "json encode");
  }

  // Route prefixes: gin/echo/fiber groups and chi's r.Route("/api", func(r chi.Router) { … }).
  const groups = new Map();
  for (const m of code.matchAll(/\b(\w+)\s*:?=\s*(\w+)\.Group\(\s*"([^"]*)"/g)) groups.set(m[1], joinRoute(groups.get(m[2]) ?? "", m[3]));
  const blocks = [...code.matchAll(/\.Route\(\s*"([^"]*)"\s*,\s*func\s*\([^)]*\)\s*\{/g)].map((m) => {
    const open = m.index + m[0].length - 1;
    return { open, close: matchClose(masked, open), prefix: m[1] };
  });
  const prefixAt = (pos, receiver) => joinRoute(...blocks.filter((b) => b.open < pos && pos < b.close).map((b) => b.prefix), groups.get(receiver) ?? "");
  const serve = (method, path, handlerExpr, pos) => {
    const p = goPayloads(code, masked, handlerFn(handlerExpr, pos));
    F.endpoints.push({ method, path, file, line: line(pos), handler: lastName(handlerExpr) || "func", request: method === "GET" || method === "DELETE" ? [] : p.decoded, response: p.encoded });
  };

  // r.GET("/x", h) (gin, echo), app.Get("/x", h) (fiber, chi), http.Get(url) / http.Post(url, …) (client).
  for (const m of code.matchAll(/\b(\w+)\.(GET|POST|PUT|PATCH|DELETE|Get|Post|Put|Patch|Delete)\s*\(/g)) {
    const a1 = argAt(code, masked, m.index + m[0].length);
    if (!a1.text) continue;
    const method = m[2].toUpperCase();
    if (m[1] === "http") {
      const p = goPayloads(code, masked, fnAt(fns, m.index));
      F.calls.push({ method, expr: a1.text, file, line: line(m.index), request: [], response: p.decoded });
      continue;
    }
    if (a1.last || !/^["`]/.test(a1.text)) continue;
    serve(method, joinRoute(prefixAt(m.index, m[1]), resolveUrlExpr(a1.text, new Map())), argAt(code, masked, a1.end + 1).text, m.index);
  }
  // net/http and gorilla/mux: HandleFunc("GET /x/{id}", h), HandleFunc("/x", h).Methods("POST").
  for (const m of code.matchAll(/\b(\w+)\.(?:HandleFunc|Handle)\s*\(\s*"(?:(GET|POST|PUT|PATCH|DELETE)\s+)?([^"]*)"\s*,/g)) {
    const a = argAt(code, masked, m.index + m[0].length);
    const tail = code.slice(a.end, a.end + 200).split("\n")[0];
    const methods = m[2] ? [m[2]] : [.../\.Methods\(([^)]*)\)/.exec(tail)?.[1].matchAll(/"(\w+)"|Method(\w+)/g) ?? []].map((x) => (x[1] ?? x[2]).toUpperCase());
    for (const method of methods.length ? methods : ["*"]) serve(method, joinRoute(prefixAt(m.index, m[1]), m[3]), a.text, m.index);
  }
  // http.NewRequest("POST", url, body) / http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
  for (const m of code.matchAll(/\bhttp\.NewRequest(WithContext)?\s*\(/g)) {
    let a = argAt(code, masked, m.index + m[0].length);
    if (m[1] && !a.last) a = argAt(code, masked, a.end + 1);
    const method = /^"(\w+)"$/.exec(a.text)?.[1]?.toUpperCase() ?? GO_METHOD[a.text.split(".").pop()];
    if (!method || a.last) continue;
    const url = argAt(code, masked, a.end + 1);
    const p = goPayloads(code, masked, fnAt(fns, m.index));
    F.calls.push({ method, expr: url.text, file, line: line(m.index), request: method === "GET" ? [] : p.encoded, response: p.decoded });
  }
  // resty: client.R().SetBody(x).SetResult(&T{}).Post(url)
  for (const m of code.matchAll(/\.R\(\)((?:\s*\.\s*\w+\((?:[^()]|\([^()]*\))*\))*)\s*\.\s*(Get|Post|Put|Patch|Delete)\s*\(/g)) {
    const a = argAt(code, masked, m.index + m[0].length);
    const result = /SetResult\(\s*&?([\w.]+)/.exec(m[1])?.[1];
    const p = goPayloads(code, masked, fnAt(fns, m.index));
    F.calls.push({ method: m[2].toUpperCase(), expr: a.text, file, line: line(m.index), request: m[2] === "Get" ? [] : p.encoded, response: result ? typeNames(result) : p.decoded });
  }
}

// Rust ------------------------------------------------------------------------------------------

/** What a Rust handler takes and returns as JSON: Json<T> extractors, Json<T> returns, .json(&x) bodies. */
function rustPayloads(code, fn) {
  if (!fn) return { request: [], response: [] };
  const body = code.slice(fn.open, fn.close);
  const vars = new Map();
  for (const m of body.matchAll(/\blet\s+(?:mut\s+)?(\w+)\s*:\s*([\w:<>]+)/g)) vars.set(m[1], m[2]);
  for (const m of body.matchAll(/\blet\s+(?:mut\s+)?(\w+)\s*=\s*([A-Z]\w*)\s*\{/g)) vars.set(m[1], m[2]);
  const request = [...fn.params.matchAll(/\bJson\s*<\s*([\w:]+(?:<[^>]*>)?)\s*>/g)].flatMap((m) => typeNames(m[1].replace(/::/g, ".")));
  let response = [...fn.returns.matchAll(/\bJson\s*<\s*([\w:]+(?:<[^>]*>)?)\s*>/g)].flatMap((m) => typeNames(m[1].replace(/::/g, ".")));
  if (!response.length) response = [...body.matchAll(/\.json\(\s*&?\s*(\w+)\s*\)|\bJson\(\s*&?\s*(\w+)\s*\)|\bJson\(\s*([A-Z]\w*)\s*\{/g)].flatMap((m) => typeNames(m[3] ?? vars.get(m[1] ?? m[2]) ?? ""));
  return { request: [...new Set(request)], response: [...new Set(response)] };
}

function extractRust(F, file, code, masked, line) {
  F.imports.set(file, [...code.matchAll(/\buse\s+(?:::)?(\w+)::([\w:]*)/g)].filter((m) => !/^(crate|self|super|std|core|alloc)$/.test(m[1])).map((m) => `${m[1]}/${m[2].replace(/::/g, "/")}`.replace(/\/$/, "")));

  for (const m of masked.matchAll(/\b(struct|enum)\s+(\w+)\s*(<[^>{;(]*>)?\s*(?:where\b[^{;]*)?\{/g)) {
    const open = m.index + m[0].length - 1;
    const prefix = prefixOf(code, masked, m.index);
    const flat = flatten(code, masked, open + 1, matchClose(masked, open));
    const fields = [];
    for (const seg of splitTop(flat)) {
      if (m[1] === "enum") {
        const em = /^((?:#\[[\s\S]*?\]\s*)*)(\w+)/.exec(seg);
        if (em && !ignored(em[1])) fields.push({ name: em[2], json: jsonName(em[1]), type: "enum" });
        continue;
      }
      const fm = /^((?:#\[[\s\S]*?\]\s*)*)(?:pub(?:\s*\([^)]*\))?\s+)?(\w+)\s*:\s*([\s\S]+)$/.exec(seg);
      if (fm && !ignored(fm[1])) fields.push({ name: fm[2], json: jsonName(fm[1]), type: tidy(fm[3]) });
    }
    if (fields.length) F.types.push({ name: m[2], kind: m[1], file, line: line(m.index), namespace: "", fields, lang: "rust", serde: /\b(Serialize|Deserialize)\b/.test(prefix) });
  }

  const fns = fnsOf(code, masked, /\bfn\s+(\w+)\s*(?:<[^>(]*>)?\s*\(/g);
  const byName = new Map(fns.map((f) => [f.name, f]));
  const serve = (method, path, handler, pos) => {
    const p = rustPayloads(code, byName.get(lastName(handler.replace(/::/g, "."))));
    F.endpoints.push({ method, path, file, line: line(pos), handler: lastName(handler), request: p.request, response: p.response });
  };
  // actix-web / rocket: #[get("/x")] async fn h(…)
  for (const m of code.matchAll(/#\[\s*(get|post|put|patch|delete)\s*\(\s*"([^"]*)"[^\]]*\]/g)) {
    const fn = fns.find((f) => f.at > m.index);
    if (fn) serve(m[1].toUpperCase(), m[2], fn.name, m.index);
  }
  // axum: .route("/x", get(list).post(create)); actix: .route("/x", web::get().to(h)), web::resource("/x").route(web::post().to(h))
  const VERBS = /\b(get|post|put|patch|delete)\s*\(\s*\)\s*\.\s*to\s*\(\s*([\w:]+)|\b(get|post|put|patch|delete)\s*\(\s*([\w:]+)\s*\)/g;
  for (const m of code.matchAll(/\.route\s*\(\s*"([^"]*)"\s*,/g)) {
    const a = argAt(code, masked, m.index + m[0].length);
    for (const v of a.text.matchAll(VERBS)) serve((v[1] ?? v[3]).toUpperCase(), m[1], v[2] ?? v[4], m.index);
  }
  for (const m of code.matchAll(/\bresource\s*\(\s*"([^"]*)"\s*\)/g)) {
    const rest = code.slice(m.index + m[0].length, m.index + 1500);
    const chain = rest.split(/\bresource\s*\(|;/)[0];
    for (const v of chain.matchAll(VERBS)) serve((v[1] ?? v[3]).toUpperCase(), m[1], v[2] ?? v[4], m.index);
  }

  // reqwest / ureq clients.
  for (const m of code.matchAll(/\b(?:(\w+)\s*\.\s*|(?:reqwest|ureq)::)(get|post|put|patch|delete)\s*\(/g)) {
    if (m[1] && /^(web|axum|routing|Router)$/.test(m[1])) continue;
    const a1 = argAt(code, masked, m.index + m[0].length);
    if (!a1.last || !/"|format!|\w+\.\w+/.test(a1.text)) continue;
    const ahead = code.slice(a1.end, a1.end + 600).split(/;\s*\n/)[0];
    const response = typeNames(/\.json\s*::\s*<\s*([\w:<>]+)\s*>/.exec(ahead)?.[1]?.replace(/::/g, ".") ?? "");
    const sent = /\.json\(\s*&?\s*(\w+)/.exec(ahead)?.[1];
    const fn = fnAt(fns, m.index);
    const vars = new Map([...(fn ? code.slice(fn.open, fn.close).matchAll(/\blet\s+(?:mut\s+)?(\w+)\s*(?::\s*([\w:<>]+))?\s*=\s*([A-Z]\w*)?/g) : [])].map((x) => [x[1], x[2] ?? x[3] ?? ""]));
    if (fn) for (const p of splitTop(fn.params)) {
      const pm = /^&?(?:mut\s+)?(\w+)\s*:\s*&?([\w:<>]+)/.exec(p);
      if (pm) vars.set(pm[1], pm[2]);
    }
    F.calls.push({ method: m[2].toUpperCase(), expr: a1.text, file, line: line(m.index), request: sent ? typeNames(vars.get(sent) ?? "") : [], response });
  }

  for (const m of code.matchAll(/serde_json::from_(?:str|slice|value|reader)\s*::\s*<\s*([\w:<>]+)\s*>|let\s+\w+\s*:\s*([\w:<>]+)\s*=\s*serde_json::from_/g))
    addUsage(F, (m[1] ?? m[2]).replace(/::/g, "."), "deserialize", file, line(m.index), "serde_json");
  for (const m of code.matchAll(/serde_json::to_(?:string|vec|value|writer)(?:_pretty)?\s*\(\s*&?\s*([A-Z]\w*)\s*\{/g)) addUsage(F, m[1], "serialize", file, line(m.index), "serde_json");
}

// Zig -------------------------------------------------------------------------------------------

function extractZig(F, file, code, masked, line) {
  F.imports.set(file, [...code.matchAll(/@import\(\s*"([^"]+)"\s*\)/g)].map((m) => m[1]).filter((x) => x !== "std" && x !== "builtin"));
  for (const m of masked.matchAll(/\bconst\s+(\w+)\s*=\s*(?:extern\s+|packed\s+)?(struct|enum|union)\s*(?:\([^)]*\))?\s*\{/g)) {
    const open = m.index + m[0].length - 1;
    const flat = flatten(code, masked, open + 1, matchClose(masked, open)).replace(/\{\}/g, "{};");
    const fields = [];
    for (const seg of splitTop(flat, ",;")) {
      if (/^(pub|fn|const|var|comptime|test|usingnamespace|inline|export|extern)\b/.test(seg)) continue;
      if (m[2] === "enum") {
        const em = /^@?"?(\w+)"?\s*(?:=.*)?$/.exec(seg);
        if (em) fields.push({ name: em[1], json: "", type: "enum" });
        continue;
      }
      const fm = /^@?"?(\w+)"?\s*:\s*([^=]+?)\s*(?:=[\s\S]*)?$/.exec(seg);
      if (fm) fields.push({ name: fm[1], json: "", type: tidy(fm[2]) });
    }
    if (fields.length) F.types.push({ name: m[1], kind: m[2], file, line: line(m.index), namespace: "", fields, lang: "zig" });
  }

  const fns = fnsOf(code, masked, /\bfn\s+(\w+)\s*\(/g);
  const byName = new Map(fns.map((f) => [f.name, f]));
  const payloads = (fn) => {
    if (!fn) return { decoded: [], encoded: [] };
    const body = code.slice(fn.open, fn.close);
    const vars = new Map([...body.matchAll(/\b(?:const|var)\s+(\w+)\s*(?::\s*([\w.]+))?\s*=\s*(?:try\s+)?([A-Z][\w.]*)?\s*\{?/g)].map((x) => [x[1], x[2] ?? x[3] ?? ""]));
    const decoded = [...body.matchAll(/\b(?:parseFromSlice|parseFromSliceLeaky|parseFromValue|parseFromTokenSource)\s*\(\s*([\w.]+)|\.json\s*\(\s*([A-Z][\w.]*)\s*\)/g)].flatMap((x) => typeNames(x[1] ?? x[2]));
    const encoded = [...body.matchAll(/\.json\s*\(\s*([A-Z][\w.]*\s*\{|\w+)\s*,|\bstringify(?:Alloc)?\s*\(\s*([A-Z][\w.]*\s*\{|\w+)\s*,|\bStringify\.value(?:Alloc)?\s*\(\s*(?:\w+\s*,\s*)?([A-Z][\w.]*\s*\{|\w+)\s*,/g)].flatMap((x) => {
      const v = (x[1] ?? x[2] ?? x[3]).replace(/\s*\{$/, "");
      return typeNames(/^[A-Z]/.test(v) ? v : (vars.get(v) ?? ""));
    });
    return { decoded: [...new Set(decoded)], encoded: [...new Set(encoded)] };
  };
  for (const fn of fns) {
    const p = payloads(fn);
    addUsage(F, p.decoded, "deserialize", file, line(fn.at), "std.json parse");
    addUsage(F, p.encoded, "serialize", file, line(fn.at), "std.json stringify");
  }
  // http.zig / zap-style routers: router.get("/x", handler, .{})
  for (const m of code.matchAll(/\b(\w+)\.(get|post|put|patch|delete)\s*\(\s*"([^"]*)"\s*,\s*([\w.]+)/g)) {
    const p = payloads(byName.get(lastName(m[4])));
    const method = m[2].toUpperCase();
    F.endpoints.push({ method, path: m[3], file, line: line(m.index), handler: lastName(m[4]), request: method === "GET" || method === "DELETE" ? [] : p.decoded, response: p.encoded });
  }
  // std.http.Client: client.fetch(.{ .method = .POST, .location = .{ .url = url }, … }) and client.open(.GET, uri, …)
  for (const m of code.matchAll(/\.fetch\s*\(\s*\.\{/g)) {
    const open = m.index + m[0].length - 1;
    const close = matchClose(masked, open);
    const opts = code.slice(open, close);
    const um = /\.(?:url|uri)\s*=\s*/.exec(opts);
    if (!um) continue;
    const start = open + um.index + um[0].length;
    const url = argAt(code, masked, start).text.replace(/\s*\}\s*$/, "");
    const p = payloads(fnAt(fns, m.index));
    F.calls.push({ method: /\.method\s*=\s*\.(\w+)/.exec(opts)?.[1]?.toUpperCase() ?? "GET", expr: url, file, line: line(m.index), request: [], response: p.decoded });
  }
  for (const m of code.matchAll(/\.open\s*\(\s*\.(GET|POST|PUT|PATCH|DELETE)\s*,\s*/g)) {
    const p = payloads(fnAt(fns, m.index));
    F.calls.push({ method: m[1], expr: argAt(code, masked, m.index + m[0].length).text, file, line: line(m.index), request: [], response: p.decoded });
  }
}

// Manifests -------------------------------------------------------------------------------------

function extractManifest(F, file, text) {
  const base = file.split("/").pop();
  if (/\.(cs|fs)proj$|\.props$|\.targets$/.test(base)) {
    for (const m of text.matchAll(/<Package(Reference|Version)\s+(?:Include|Update)\s*=\s*"([^"]+)"([^>]*?)(?:\/>|>([\s\S]*?)<\/Package(?:Reference|Version)>)/g)) {
      const version = /Version\s*=\s*"([^"]*)"/.exec(m[3])?.[1] ?? /<Version>([^<]*)<\/Version>/.exec(m[4] ?? "")?.[1] ?? "";
      if (m[1] === "Version") F.centralVersions.set(m[2].toLowerCase(), version);
      else F.packages.push({ name: m[2], version, file, ecosystem: "nuget" });
    }
    if (/proj$/.test(base)) {
      const id = /<PackageId>\s*([^<$]+?)\s*<\/PackageId>/.exec(text)?.[1] ?? /<AssemblyName>\s*([^<$]+?)\s*<\/AssemblyName>/.exec(text)?.[1] ?? base.replace(/\.\w+proj$/, "");
      F.provides.push({ name: id, file, ecosystem: "nuget", version: /<(?:Package)?Version>\s*([^<$]+?)\s*<\//.exec(text)?.[1] ?? "" });
    }
  } else if (base === "packages.config") {
    for (const m of text.matchAll(/<package\s+id="([^"]+)"\s+version="([^"]*)"/g)) F.packages.push({ name: m[1], version: m[2], file, ecosystem: "nuget" });
  } else if (base === "package.json") {
    let json;
    try {
      json = JSON.parse(text);
    } catch {
      return;
    }
    if (json?.name) F.provides.push({ name: String(json.name), file, ecosystem: "npm", version: String(json.version ?? "") });
    for (const section of ["dependencies", "devDependencies", "peerDependencies"])
      for (const [name, version] of Object.entries(json?.[section] ?? {})) F.packages.push({ name, version: String(version), file, ecosystem: "npm" });
  } else if (base === "pubspec.yaml") {
    const name = /^name:\s*(\S+)/m.exec(text)?.[1];
    if (name) F.provides.push({ name, file, ecosystem: "pub", version: /^version:\s*(\S+)/m.exec(text)?.[1] ?? "" });
    let inDeps = false;
    for (const row of text.split("\n")) {
      if (/^\S/.test(row)) inDeps = /^(dev_)?dependencies:/.test(row);
      else if (inDeps) {
        const dm = /^ {2}([\w-]+):\s*(.*)$/.exec(row);
        if (dm) F.packages.push({ name: dm[1], version: dm[2].trim(), file, ecosystem: "pub" });
      }
    }
  } else if (base === "go.mod") {
    const mod = /^\s*module\s+(\S+)/m.exec(text)?.[1];
    if (mod) F.provides.push({ name: mod, file, ecosystem: "go", version: "" });
    for (const m of text.matchAll(/^\s*(?:require\s+)?([\w.~-]+\.[\w.~/-]+)\s+(v[\w.+-]+)/gm)) F.packages.push({ name: m[1], version: m[2], file, ecosystem: "go" });
  } else if (base === "Cargo.toml") {
    // Crates are imported with "_" for "-", so names are kept that way.
    const crate = (name) => name.replace(/-/g, "_");
    let section = "";
    for (const row of text.split("\n")) {
      const head = /^\s*\[([^\]]+)\]\s*$/.exec(row);
      if (head) {
        section = head[1].trim();
        const table = /(?:^|\.)(?:dev-|build-)?dependencies\.([\w-]+)$/.exec(section);
        if (table) F.packages.push({ name: crate(table[1]), version: "", file, ecosystem: "cargo" });
        continue;
      }
      const kv = /^\s*([\w.-]+)\s*=\s*(.+?)\s*$/.exec(row);
      if (!kv) continue;
      const version = /^"([^"]*)"/.exec(kv[2])?.[1] ?? /\bversion\s*=\s*"([^"]*)"/.exec(kv[2])?.[1] ?? "";
      if (section === "package") {
        if (kv[1] === "name") F.provides.push({ name: crate(/"([^"]*)"/.exec(kv[2])?.[1] ?? ""), file, ecosystem: "cargo", version: "" });
        else if (kv[1] === "version") for (const pr of F.provides) if (pr.file === file && !pr.version) pr.version = version;
      } else if (section === "workspace.dependencies") F.centralVersions.set(crate(kv[1]).toLowerCase(), version);
      else if (/(?:^|\.)(?:dev-|build-)?dependencies$/.test(section)) F.packages.push({ name: crate(kv[1].replace(/\.workspace$/, "")), version, file, ecosystem: "cargo" });
      else if (/(?:^|\.)(?:dev-|build-)?dependencies\.[\w-]+$/.test(section) && kv[1] === "version") {
        const last = F.packages.at(-1);
        if (last?.file === file) last.version = version;
      }
    }
  } else if (base === "build.zig.zon") {
    const name = /\.name\s*=\s*(?:"([^"]+)"|\.@?"?(\w+)"?)/.exec(text);
    if (name) F.provides.push({ name: name[1] ?? name[2], file, ecosystem: "zig", version: /\.version\s*=\s*"([^"]*)"/.exec(text)?.[1] ?? "" });
    const deps = /\.dependencies\s*=\s*\.\{/.exec(text);
    if (deps) {
      const { masked } = lex(text, "zig");
      const open = deps.index + deps[0].length - 1;
      const close = matchClose(masked, open);
      for (const m of text.slice(open + 1, close).matchAll(/\.@?"?([\w-]+)"?\s*=\s*\.\{([^}]*)\}/g))
        F.packages.push({ name: m[1], version: /\.url\s*=\s*"[^"]*?v?(\d+\.\d+\.\d+[\w.-]*?)(?:\.tar\.gz|\.zip|\.tgz)?"/.exec(m[2])?.[1] ?? "", file, ecosystem: "zig" });
    }
  } else if (/^build\.gradle(\.kts)?$/.test(base)) {
    for (const m of text.matchAll(/["']([\w.-]+:[\w.-]+):([^"'$\s]+)["']/g)) F.packages.push({ name: m[1], version: m[2], file, ecosystem: "maven" });
  }
}

const MANIFEST = /(?:^|\/)(?:package\.json|pubspec\.yaml|packages\.config|Directory\.Packages\.props|Directory\.Build\.props|build\.gradle(?:\.kts)?|go\.mod|Cargo\.toml|build\.zig\.zon)$|\.(?:cs|fs)proj$/;

/** Adds one file to a project's facts. */
export function extractFile(F, file, text) {
  if (MANIFEST.test(file)) {
    extractManifest(F, file, text);
    return;
  }
  if (SPEC_FILE.test(file)) {
    extractSpec(F, file, text);
    return;
  }
  const lang = LANGS[/\.(\w+)$/.exec(file)?.[1]?.toLowerCase() ?? ""];
  if (!lang) return;
  F.files++;
  F.langs[lang] = (F.langs[lang] ?? 0) + 1;
  const { code, masked } = lex(text, lang);
  const line = lineIndex(text);
  const seen = new Map();
  for (const m of masked.matchAll(/\b[A-Z][A-Za-z0-9_]{2,}\b/g)) {
    const n = seen.get(m[0]) ?? 0;
    if (n >= 3) continue;
    seen.set(m[0], n + 1);
    let list = F.tokens.get(m[0]);
    if (!list) F.tokens.set(m[0], (list = []));
    const at = line(m.index);
    if (list.length < 120 && !list.some((x) => x.file === file && x.line === at)) list.push({ file, line: at });
  }
  scanStrings(F, file, code, line);
  if (lang === "csharp") extractCSharp(F, file, code, masked, line);
  else if (lang === "ts") extractTs(F, file, code, masked, line);
  else if (lang === "kotlin" || lang === "java") extractJvm(F, file, code, masked, line, lang);
  else if (lang === "swift") extractSwift(F, file, code, masked, line);
  else if (lang === "dart") extractDart(F, file, code, masked, line);
  else if (lang === "go") extractGo(F, file, code, masked, line);
  else if (lang === "rust") extractRust(F, file, code, masked, line);
  else if (lang === "zig") extractZig(F, file, code, masked, line);
}

/** Resolves call URLs, fills central package versions and records what endpoints and calls carry. */
export function finishFacts(F) {
  for (const p of F.packages) if (!p.version) p.version = F.centralVersions.get(p.name.toLowerCase()) ?? "";
  F.calls = F.calls
    .map((c) => {
      const info = {};
      const raw = resolveUrlExpr(c.expr, F.consts, info);
      return { ...c, path: raw, segs: normPath(raw), expr: undefined, known: info.known };
    })
    .filter((c) => c.known && c.path.includes("/") && !FILE_LIKE.test(c.path.split(/[?#]/)[0]) && c.segs.some((x) => x !== "{}"))
    .map(({ known, ...c }) => c);
  F.endpoints = F.endpoints.map(({ route, ...e }) => {
    const path = route ? controllerPath(F, route) : e.path;
    return { ...e, path, segs: normPath(path) };
  });
  specClients(F);
  for (const e of F.endpoints) {
    addUsage(F, e.response, "serialize", e.file, e.line, `${e.method} /${e.segs.join("/")} response`);
    addUsage(F, e.request, "deserialize", e.file, e.line, `${e.method} /${e.segs.join("/")} request body`);
  }
  for (const c of F.calls) {
    addUsage(F, c.response, "deserialize", c.file, c.line, `${c.method} /${c.segs.join("/")} response`);
    addUsage(F, c.request, "serialize", c.file, c.line, `${c.method} /${c.segs.join("/")} request body`);
  }
  const seen = new Set();
  F.usages = F.usages.filter((u) => {
    const key = `${u.type}|${u.mode}|${u.file}|${u.line}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  return F;
}

/** Splits the scanner's dump ("\n@@FILE path\n<content>" per file) and extracts every file. */
export function factsFromDump(dump) {
  const F = newFacts();
  const parts = String(dump).split(/\n@@FILE /);
  for (const part of parts.slice(1)) {
    const nl = part.indexOf("\n");
    const file = (nl < 0 ? part : part.slice(0, nl)).trim().replace(/\\/g, "/").replace(/^\.\//, "");
    try {
      extractFile(F, file, nl < 0 ? "" : part.slice(nl + 1));
    } catch {
      // One odd file shouldn't stop the scan.
    }
  }
  return finishFacts(F);
}

// Linking ---------------------------------------------------------------------------------------

/** How well endpoint segments `e` match call segments `c`: aligned from the end, {} matches anything. */
export function pathScore(e, c) {
  const k = Math.min(e.length, c.length);
  if (!k) return 0;
  let literal = 0;
  let meaningful = 0;
  for (let i = 1; i <= k; i++) {
    const a = e[e.length - i];
    const b = c[c.length - i];
    if (a === b) {
      if (a !== "{}") {
        literal++;
        if (!/^(api|v\d+|rest)$/.test(a)) meaningful++;
      }
    } else if (a !== "{}" && b !== "{}") return 0;
  }
  if (!meaningful) return 0;
  return literal * 2 + (k - literal) - Math.abs(e.length - c.length) * 0.5;
}

const sameMethod = (a, b) => !a || !b || a === "*" || b === "*" || a === "?" || b === "?" || a === b;
const cleanVersion = (v) => String(v ?? "").replace(/^[\^~=v\s[(]+|[\])\s]+$/g, "").trim();

/** Which package of `pkgs` (lowercase names) an import or namespace belongs to. */
function packageOfImport(imp, pkgs) {
  const s = imp.toLowerCase();
  let best = "";
  for (const p of pkgs) if ((s === p || s.startsWith(`${p}.`) || s.startsWith(`${p}/`) || p.startsWith(`${s}.`)) && p.length > best.length) best = p;
  return best;
}

/** Compares a contract's definitions field by field. */
export function compareFields(defs) {
  const roots = defs.map((d) => d.key);
  const rows = new Map();
  for (const d of defs)
    for (const f of d.fields) {
      const key = normKey(f.json || f.name);
      if (!rows.has(key)) rows.set(key, { key, cells: {} });
      rows.get(key).cells[d.key] ??= { name: f.name, json: f.json, type: f.type };
    }
  let problems = 0;
  const list = [...rows.values()].map((row) => {
    const missing = roots.filter((r) => !row.cells[r]);
    const families = [...new Set(Object.values(row.cells).map((c) => typeFamily(c.type)).filter(Boolean))];
    const issue = missing.length ? "missing" : families.length > 1 ? "type" : null;
    if (issue) problems++;
    return { ...row, issue, missing };
  });
  list.sort((a, b) => (a.issue ? 0 : 1) - (b.issue ? 0 : 1) || a.key.localeCompare(b.key));
  return { roots, rows: list, problems };
}

/**
 * Links the projects of a system. `projects` is [{ root, name, facts }]; returns plain data
 * (paths relative to each root) for the page, the agent tools and localStorage.
 */
export function linkSystem(projects) {
  const live = projects.filter((p) => p.facts);
  const byRoot = new Map(live.map((p) => [p.root, p]));

  // Packages.
  const pkgMap = new Map();
  const pkg = (name, ecosystem) => {
    const key = name.toLowerCase();
    if (!pkgMap.has(key)) pkgMap.set(key, { name, ecosystem, uses: {}, providers: [], contracts: 0 });
    return pkgMap.get(key);
  };
  for (const p of live) {
    for (const u of p.facts.packages) (pkg(u.name, u.ecosystem).uses[p.root] ??= []).push({ version: u.version, file: u.file });
    for (const pr of p.facts.provides) {
      const entry = pkg(pr.name, pr.ecosystem);
      if (!entry.providers.some((x) => x.root === p.root)) entry.providers.push({ root: p.root, version: pr.version, file: pr.file });
    }
  }
  const packages = [...pkgMap.values()]
    .map((x) => {
      const roots = new Set([...Object.keys(x.uses), ...x.providers.map((pr) => pr.root)]);
      const versions = new Set([...Object.values(x.uses).flatMap((list) => list.map((u) => cleanVersion(u.version))), ...x.providers.map((pr) => cleanVersion(pr.version))].filter((v) => v && !/^\$|\*/.test(v)));
      return { ...x, roots: [...roots], versions: [...versions], drift: versions.size > 1 };
    })
    .filter((x) => x.roots.length > 1);
  const sharedNames = new Set(packages.map((x) => x.name.toLowerCase()));

  // Contract candidates: models with fields, and anything (de)serialized or carried by an endpoint.
  const candidates = new Set();
  for (const p of live) {
    for (const t of p.facts.types) if (t.fields.length && !STOP.has(t.name)) candidates.add(t.name);
    for (const u of p.facts.usages) if (!STOP.has(u.type)) candidates.add(u.type);
  }
  const contracts = [];
  for (const name of candidates) {
    const per = {};
    let strong = false;
    for (const p of live) {
      const defines = p.facts.types.filter((t) => t.name === name);
      const usages = p.facts.usages.filter((u) => u.type === name);
      const ser = usages.filter((u) => u.mode === "serialize");
      const deser = usages.filter((u) => u.mode === "deserialize");
      const defLines = new Set(defines.map((d) => `${d.file}:${d.line}`));
      const useLines = new Set(usages.map((u) => `${u.file}:${u.line}`));
      const refs = (p.facts.tokens.get(name) ?? []).filter((r) => !defLines.has(`${r.file}:${r.line}`) && !useLines.has(`${r.file}:${r.line}`) && !defines.some((d) => d.file === r.file));
      if (!defines.length && !usages.length && !refs.length) continue;
      if (ser.length || deser.length) strong = true;
      per[p.root] = {
        defines: defines.map((d) => ({ file: d.file, line: d.line, kind: d.kind, namespace: d.namespace, lang: d.lang, fields: d.fields })),
        ser: ser.slice(0, MAX_SITES).map(({ file, line, via }) => ({ file, line, via })),
        deser: deser.slice(0, MAX_SITES).map(({ file, line, via }) => ({ file, line, via })),
        refs: refs.slice(0, MAX_SITES),
        counts: { ser: ser.length, deser: deser.length, refs: refs.length },
      };
    }
    const roots = Object.keys(per);
    if (roots.length < 2) continue;
    const definedIn = roots.filter((r) => per[r].defines.some((d) => d.fields.length));
    if (!strong && !definedIn.length) continue;
    // A model defined in one project and mentioned once by name in another is too thin to show,
    // unless a shared contract has it as a field (see below).
    const enough = definedIn.length >= 2 || roots.some((r) => per[r].defines.length === 0 && per[r].counts.refs >= 2);

    // Which shared package it comes from: imports/namespaces of the files that use it.
    const votes = new Map();
    for (const r of roots) {
      const facts = byRoot.get(r).facts;
      const files = new Set([...per[r].defines, ...per[r].ser, ...per[r].deser, ...per[r].refs].map((x) => x.file));
      const own = facts.provides.map((pr) => pr.name.toLowerCase()).filter((n) => sharedNames.has(n));
      for (const d of per[r].defines) {
        const hit = packageOfImport(d.namespace || "", own);
        if (hit) votes.set(hit, (votes.get(hit) ?? 0) + 3);
      }
      const used = facts.packages.map((u) => u.name.toLowerCase()).filter((n) => sharedNames.has(n));
      for (const file of files)
        for (const imp of facts.imports.get(file) ?? []) {
          const hit = packageOfImport(imp, used);
          if (hit) votes.set(hit, (votes.get(hit) ?? 0) + 1);
        }
    }
    const top = [...votes.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
    const packageName = top ? pkgMap.get(top).name : "";
    if (packageName) strong = true;

    const defs = definedIn.map((r) => ({ key: r, fields: per[r].defines.find((d) => d.fields.length).fields }));
    const fields = defs.length > 1 ? compareFields(defs) : null;
    if (fields && !strong && fields.rows.filter((row) => !row.issue).length >= Math.max(1, fields.rows.length / 2)) strong = true;
    contracts.push({ name, package: packageName, strength: strong ? "strong" : "weak", enough, roots, per, fields, problems: fields?.problems ?? 0, apis: [] });
  }
  // Nested models (OrderDto.Lines → OrderLineDto, OrderDto.Status → OrderStatus) travel with their parent.
  for (let changed = true; changed; ) {
    changed = false;
    const nested = new Set(contracts.filter((c) => c.strength === "strong").flatMap((c) => Object.values(c.per).flatMap((x) => x.defines.flatMap((d) => d.fields.flatMap((f) => typeNames(f.type))))));
    for (const c of contracts)
      if (c.strength === "weak" && nested.has(c.name)) {
        c.strength = "strong";
        changed = true;
      }
  }
  for (let i = contracts.length - 1; i >= 0; i--) {
    if (contracts[i].strength === "weak" && !contracts[i].enough) contracts.splice(i, 1);
    else delete contracts[i].enough;
  }
  for (const c of contracts) if (c.package) packages.find((x) => x.name === c.package).contracts++;

  // HTTP: each client call to the best matching endpoint in another project.
  const endpoints = live.flatMap((p) => p.facts.endpoints.map((e) => ({ ...e, root: p.root })));
  const apis = new Map();
  const unmatched = [];
  const findTarget = (call, root) => {
    let best = null;
    let bestScore = 0;
    for (const e of endpoints) {
      if (e.root === root || !sameMethod(e.method, call.method)) continue;
      const path = pathScore(e.segs, call.segs);
      if (path <= 0) continue;
      const score = path + (e.method === call.method ? 0.25 : 0);
      if (score > bestScore) {
        best = e;
        bestScore = score;
      }
    }
    return best;
  };
  const link = (e, client) => {
    const id = `${e.root}|${e.method}|${e.segs.join("/")}`;
    if (!apis.has(id))
      apis.set(id, { id, method: e.method, path: `/${e.segs.join("/")}`, server: { root: e.root, file: e.file, line: e.line, handler: e.handler, request: e.request, response: e.response }, clients: [], issues: [] });
    const api = apis.get(id);
    if (!api.clients.some((x) => x.root === client.root && x.file === client.file && x.line === client.line)) api.clients.push(client);
  };
  for (const p of live) {
    const explicit = new Set();
    for (const c of p.facts.calls) {
      explicit.add(`${c.file}:${c.line}`);
      const e = findTarget(c, p.root);
      if (e) link(e, { root: p.root, file: c.file, line: c.line, method: c.method, path: c.path, request: c.request, response: c.response });
      else if (endpoints.some((x) => x.root !== p.root) && unmatched.length < 200) unmatched.push({ root: p.root, method: c.method, path: c.path, file: c.file, line: c.line });
    }
    // Path literals elsewhere (constants, URL builders) count only when they match an endpoint.
    for (const l of p.facts.literals) {
      const segs = normPath(l.path);
      if (explicit.has(`${l.file}:${l.line}`) || p.facts.calls.some((c) => c.path.includes(l.path) || (c.file === l.file && c.segs.join("/") === segs.join("/")))) continue;
      if (segs.length < 2) continue;
      const e = findTarget({ method: "?", segs }, p.root);
      if (e && pathScore(e.segs, segs) >= 4) link(e, { root: p.root, file: l.file, line: l.line, method: "?", path: l.path, request: [], response: [], literal: true });
    }
  }
  // Both sides of an endpoint should agree on what's sent.
  const defOf = (root, name) => byRoot.get(root)?.facts.types.find((t) => t.name === name && t.fields.length);
  for (const api of apis.values()) {
    for (const client of api.clients) {
      for (const [side, a, b] of [
        ["response", api.server.response, client.response],
        ["request", api.server.request, client.request],
      ]) {
        if (!a.length || !b.length || a.some((x) => b.includes(x))) continue;
        const da = defOf(api.server.root, a[0]);
        const db = defOf(client.root, b[0]);
        const cmp = da && db ? compareFields([{ key: api.server.root, fields: da.fields }, { key: client.root, fields: db.fields }]) : null;
        api.issues.push({ side, server: a[0], client: b[0], clientRoot: client.root, fields: cmp });
      }
    }
    for (const c of contracts) {
      const carried = [api.server.request, api.server.response, ...api.clients.flatMap((x) => [x.request, x.response])].some((list) => list.includes(c.name));
      if (carried) c.apis.push(api.id);
    }
  }

  const apiList = [...apis.values()].sort((a, b) => a.path.localeCompare(b.path) || a.method.localeCompare(b.method));
  contracts.sort((a, b) => (a.strength === b.strength ? 0 : a.strength === "strong" ? -1 : 1) || b.problems - a.problems || b.roots.length - a.roots.length || a.name.localeCompare(b.name));
  packages.sort((a, b) => b.contracts - a.contracts || Number(b.drift) - Number(a.drift) || b.roots.length - a.roots.length || a.name.localeCompare(b.name));
  return {
    scannedAt: Date.now(),
    projects: projects.map((p) => ({
      root: p.root,
      name: p.name,
      error: p.error ?? "",
      files: p.facts?.files ?? 0,
      langs: p.facts?.langs ?? {},
      types: p.facts?.types.length ?? 0,
      endpoints: p.facts?.endpoints.length ?? 0,
      calls: p.facts?.calls.length ?? 0,
    })),
    contracts,
    packages: packages.map(({ name, ecosystem, uses, providers, roots, versions, drift, contracts: n }) => ({ name, ecosystem, uses, providers, roots, versions, drift, contracts: n })),
    apis: apiList,
    unmatched,
  };
}

// Problems --------------------------------------------------------------------------------------

/** Everything worth a look in a scan, most serious first. */
export function problemsOf(result, names = {}) {
  const name = (root) => names[root] ?? baseName(root);
  const out = [];
  for (const c of result?.contracts ?? []) {
    if (!c.fields?.problems) continue;
    const missing = c.fields.rows.filter((r) => r.issue === "missing");
    const types = c.fields.rows.filter((r) => r.issue === "type");
    const parts = [];
    if (missing.length) parts.push(`${missing.map((r) => `${Object.values(r.cells)[0].json || Object.values(r.cells)[0].name} (not in ${r.missing.map(name).join(", ")})`).slice(0, 4).join(", ")}${missing.length > 4 ? ` +${missing.length - 4}` : ""}`);
    if (types.length) parts.push(`different types: ${types.map((r) => `${Object.values(r.cells)[0].name} (${Object.entries(r.cells).map(([root, cell]) => `${cell.type} in ${name(root)}`).join(" vs ")})`).slice(0, 3).join(", ")}`);
    out.push({ kind: "fields", weight: 3, title: `${c.name}: ${plural(c.fields.problems, "field")} differ`, detail: parts.join("; "), select: { lens: "contracts", id: c.name } });
  }
  for (const a of result?.apis ?? [])
    for (const issue of a.issues) {
      const diff = issue.fields?.problems ? `, ${plural(issue.fields.problems, "field")} differ` : "";
      out.push({ kind: "api", weight: issue.fields?.problems ? 3 : 1, title: `${a.method} ${a.path}: ${issue.side} is ${issue.server} on the server, ${issue.client} in ${name(issue.clientRoot)}${diff}`, detail: "", select: { lens: "apis", id: a.id } });
    }
  for (const p of result?.packages ?? []) {
    if (!p.drift) continue;
    const by = new Map();
    for (const [root, list] of Object.entries(p.uses)) for (const u of list) if (cleanVersion(u.version)) by.set(cleanVersion(u.version), [...new Set([...(by.get(cleanVersion(u.version)) ?? []), name(root)])]);
    for (const pr of p.providers) if (cleanVersion(pr.version)) by.set(cleanVersion(pr.version), [...new Set([...(by.get(cleanVersion(pr.version)) ?? []), `${name(pr.root)} (publishes it)`])]);
    out.push({ kind: "package", weight: p.contracts ? 2 : 0.5, title: `${p.name}: different versions`, detail: [...by.entries()].map(([v, who]) => `${v} in ${who.join(", ")}`).join("; "), select: { lens: "packages", id: p.name } });
  }
  for (const u of result?.unmatched ?? []) out.push({ kind: "unmatched", weight: 0.3, title: `${u.method} ${u.path} in ${name(u.root)} matches no endpoint`, detail: `${u.file}:${u.line}`, select: { project: u.root } });
  return out.sort((a, b) => b.weight - a.weight);
}

// Helpers ---------------------------------------------------------------------------------------

/** Builds an element: h("div", { class: "x", onclick }, child, "text", …). */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else if (key in el && typeof value !== "string") el[key] = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

const SVG_NS = "http://www.w3.org/2000/svg";
function s(tag, attrs = {}, ...children) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, String(value));
  }
  for (const child of children.flat()) if (child !== null && child !== undefined && child !== false) el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  return el;
}

const ICONS = {
  refresh: '<path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
  link: '<path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7"/>',
  box: '<path d="m12 3 8 4.5v9L12 21l-8-4.5v-9z"/><path d="m4 7.5 8 4.5 8-4.5M12 12v9"/>',
  plug: '<path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4"/>',
  braces: '<path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5a2 2 0 0 0 2 2h1M16 3h1a2 2 0 0 1 2 2v5a2 2 0 0 0 2 2 2 2 0 0 0-2 2v5a2 2 0 0 1-2 2h-1"/>',
  alert: '<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/><path d="M12 9v4M12 17h.01"/>',
  check: '<path d="m5 12 5 5 9-10"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>',
  external: '<path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/>',
  bot: '<rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 4v4"/><circle cx="9" cy="14" r="1"/><circle cx="15" cy="14" r="1"/>',
  folder: '<path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  arrowLeft: '<path d="M19 12H5M11 6l-6 6 6 6"/>',
};

function icon(name, cls = "xp-i") {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", cls);
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.innerHTML = ICONS[name] ?? "";
  return svg;
}

function q(value) {
  const str = String(value ?? "").replace(/\0/g, "");
  if (IS_WIN) return `'${str.replace(/['\u2018\u2019\u201a\u201b]/g, (m) => m + m)}'`;
  return `'${str.replace(/'/g, `'\\''`)}'`;
}

/** Wraps a PowerShell script so it reaches PowerShell intact through `cmd /C` (no quotes at all). */
function powershell(script) {
  const text = `$ProgressPreference = 'SilentlyContinue'; [Console]::OutputEncoding = [Text.Encoding]::UTF8; $OutputEncoding = [Text.Encoding]::UTF8; $env:PYTHONIOENCODING = 'utf-8'; $env:PYTHONUTF8 = '1'\n${script}`;
  let bytes = "";
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    bytes += String.fromCharCode(c & 0xff, c >> 8);
  }
  return `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${btoa(bytes)}`;
}

const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
function plural(n, word) {
  return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
}
function baseName(p) {
  return String(p).split(/[\\/]/).filter(Boolean).pop() ?? String(p);
}
const truncate = (text, max) => (text.length > max ? `${text.slice(0, max)}\n… (cut at ${max.toLocaleString()} characters)` : text);
const lastLines = (text, n = 6) => String(text).trim().split("\n").slice(-n).join("\n");
const joinPath = (root, rel) => `${String(root).replace(/[\\/]+$/, "")}/${rel}`;
const fileUrl = (absolute) => (IS_WIN ? `file:///${encodeURI(absolute.replace(/\\/g, "/"))}` : `file://${encodeURI(absolute)}`);
const isLight = () => document.documentElement.dataset.mode === "light";

function ago(ms) {
  if (!ms) return "never";
  const sec = Math.max(0, (Date.now() - ms) / 1000);
  if (sec < 45) return "just now";
  if (sec < 3600) return `${Math.round(sec / 60)} min ago`;
  if (sec < 86_400) return `${Math.round(sec / 3600)} h ago`;
  return `${Math.round(sec / 86_400)} d ago`;
}

/** OKLCH to an sRGB css color, so every project color looks equally bright. */
function oklch(L, C, hueDeg) {
  const hr = (hueDeg * Math.PI) / 180;
  const a = C * Math.cos(hr);
  const b = C * Math.sin(hr);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const k = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const enc = (x) => {
    x = clamp(x, 0, 1);
    return Math.round(255 * (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055));
  };
  return `rgb(${enc(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * k)} ${enc(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * k)} ${enc(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * k)})`;
}
const projectColor = (i) => oklch(isLight() ? 0.58 : 0.76, isLight() ? 0.16 : 0.14, (i * 137.508 + 200) % 360);

// Scanning --------------------------------------------------------------------------------------

const INCLUDE =
  "\\.(cs|ts|tsx|js|jsx|mjs|cjs|kt|java|swift|dart|go|rs|zig)$|(^|/)(package\\.json|pubspec\\.yaml|packages\\.config|Directory\\.Packages\\.props|Directory\\.Build\\.props|build\\.gradle|build\\.gradle\\.kts|go\\.mod|Cargo\\.toml|build\\.zig\\.zon)$|\\.(cs|fs)proj$" +
  "|([Ss]wagger|[Oo]pen[Aa][Pp][Ii]|[Aa]pi[-_.]?[Ss]pec)[^/]*\\.(json|ya?ml)$|(^|/)[Oo]pen[Aa][Pp][Ii]s/[^/]+\\.(json|ya?ml)$|\\.nswag$|(^|/)nswag\\.json$";
const EXCLUDE =
  "(^|/)(node_modules|bin|obj|dist|build|out|Pods|Carthage|\\.gradle|\\.dart_tool|DerivedData|\\.next|\\.nuxt|\\.svelte-kit|coverage|vendor|graphify-out|\\.git|Migrations|target|zig-out|\\.?zig-cache|swagger-ui)/|wwwroot/lib/|\\.min\\.js$|\\.bundle\\.js$|_test\\.go$|\\.g\\.i\\.cs$|\\.Designer\\.cs$|\\.d\\.ts$|\\.(spec|test)\\.[cm]?[jt]sx?$|AssemblyInfo\\.cs$";

/** Prints "\n@@FILE <path>\n<content>" for every source file and manifest of the project. */
const DUMP_UNIX = [
  "{ if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then git -c core.quotePath=false ls-files -co --exclude-standard;",
  "else find . -type d \\( -name node_modules -o -name .git -o -name bin -o -name obj -o -name Pods -o -name build -o -name dist -o -name target -o -name zig-out -o -name .zig-cache -o -name zig-cache \\) -prune -o -type f -print | sed 's|^\\./||'; fi; }",
  `| grep -E ${q(INCLUDE)} | grep -vE ${q(EXCLUDE)}`,
  `| { t=0; while IFS= read -r f; do [ -f "$f" ] || continue; n=$(($(wc -c < "$f"))); if [ "$n" -gt ${MAX_FILE} ]; then [ "$n" -le ${MAX_GENERATED} ] && printf '%s' "$f" | grep -qE ${q(BIG)} || continue; fi; t=$((t + n)); [ "$t" -le ${MAX_TOTAL} ] || break; printf '\\n@@FILE %s\\n' "$f"; cat "$f"; done; }`,
].join(" ");
const DUMP_WIN = [
  `$inc = ${q(INCLUDE)}; $exc = ${q(EXCLUDE)}; $big = ${q(BIG)}`,
  "$files = @(git -c core.quotePath=false ls-files -co --exclude-standard 2>$null)",
  "if ($LASTEXITCODE -ne 0 -or $files.Count -eq 0) { $base = (Get-Location).Path; $files = @(Get-ChildItem -Recurse -File -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName.Substring($base.Length + 1).Replace('\\', '/') }) }",
  "$sb = New-Object Text.StringBuilder; $total = 0",
  `foreach ($f in $files) { if ($f -match $inc -and $f -notmatch $exc) { $p = Join-Path (Get-Location) $f; if (Test-Path -LiteralPath $p -PathType Leaf) { $len = (Get-Item -LiteralPath $p).Length; if ($len -le ${MAX_FILE} -or ($len -le ${MAX_GENERATED} -and $f -cmatch $big)) { $total += $len; if ($total -gt ${MAX_TOTAL}) { break }; [void]$sb.Append("\`n@@FILE $f\`n"); [void]$sb.Append([IO.File]::ReadAllText($p)) } } } }`,
].join("\n");

/** Runs `script` (which prints text), gzips and base64-encodes its output into a temp file; prints "<size> <file>". */
const packed = (script) =>
  IS_WIN
    ? `${script}\n$b = [Text.Encoding]::UTF8.GetBytes($sb.ToString()); $m = New-Object IO.MemoryStream; $z = New-Object IO.Compression.GZipStream($m, [IO.Compression.CompressionMode]::Compress); $z.Write($b, 0, $b.Length); $z.Close(); $s = [Convert]::ToBase64String($m.ToArray()); $f = [IO.Path]::GetTempFileName(); [IO.File]::WriteAllText($f, $s); [string]$s.Length + ' ' + $f`
    : `f=$(mktemp) && { ${script}; } > "$f.raw" 2>/dev/null; gzip -c "$f.raw" | base64 | tr -d '\\n' > "$f"; rm -f "$f.raw"; echo "$(wc -c < "$f" | tr -d ' ') $f"`;

const GRAPHIFY_UNIX = 'b=$(command -v graphify 2>/dev/null); if [ -z "$b" ]; then for p in "$HOME/.local/bin/graphify" "$HOME/.local/share/uv/tools/graphifyy/bin/graphify"; do if [ -x "$p" ]; then b="$p"; break; fi; done; fi; echo "$b"';
const GRAPHIFY_WIN = '$c = Get-Command graphify -ErrorAction SilentlyContinue; if ($c) { $c.Source } elseif (Test-Path "$env:USERPROFILE\\.local\\bin\\graphify.exe") { "$env:USERPROFILE\\.local\\bin\\graphify.exe" }';

/** Systems (groups of linked projects), their last scan and what's scanning now. */
function createStore(api) {
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem(STORAGE) ?? "{}") ?? {};
  } catch {
    saved = {};
  }
  const systems = Array.isArray(saved.systems) ? saved.systems : [];
  const settings = { autoRescan: true, motion: !REDUCED_MOTION, showWeak: false, ...saved.settings };
  let activeId = saved.activeId ?? systems[0]?.id ?? null;
  const results = new Map();
  const scans = new Map();
  const listeners = new Set();
  const timers = new Map();

  const changed = () => listeners.forEach((fn) => fn());
  const save = () => {
    try {
      localStorage.setItem(STORAGE, JSON.stringify({ systems, settings, activeId }));
    } catch {
      // Out of space: settings stay for this session.
    }
  };
  const resultKey = (id) => `${STORAGE}.result.${id}`;
  function result(id) {
    if (!results.has(id)) {
      try {
        results.set(id, JSON.parse(localStorage.getItem(resultKey(id)) ?? "null"));
      } catch {
        results.set(id, null);
      }
    }
    return results.get(id);
  }

  let home = null;
  async function absolute(dir) {
    if (dir !== "~" && !dir.startsWith("~/") && !dir.startsWith("~\\")) return dir;
    if (home === null) home = (await api.vault.exec({ command: IS_WIN ? "echo %USERPROFILE%" : 'echo "$HOME"', cwd: "", keys: [], timeoutSec: 15 })).output.trim();
    return home + dir.slice(1);
  }
  const exec = async (cwd, command, timeoutSec = 120) => {
    const r = await api.vault.exec({ command: IS_WIN ? powershell(command) : command, cwd: await absolute(cwd), keys: [], timeoutSec });
    return { ...r, output: r.output.replace(/\r\n/g, "\n") };
  };

  /** Runs a command whose output may be far bigger than vault.exec returns, by reading it back in chunks. */
  async function readPacked(root, script, onProgress) {
    const r = await exec(root, packed(script), 600);
    const match = /^(\d+) (.+)$/m.exec(r.output.trim());
    if (r.timedOut) throw new Error("Reading the project took too long");
    if (!match) throw new Error(lastLines(r.output, 3) || `exit code ${r.code}`);
    const size = Number(match[1]);
    const file = match[2].trim();
    const chunk = (offset) => (IS_WIN ? `[IO.File]::ReadAllText(${q(file)}).Substring(${offset}, ${Math.min(CHUNK, size - offset)})` : `tail -c +${offset + 1} ${q(file)} | head -c ${CHUNK}`);
    try {
      const offsets = [];
      for (let offset = 0; offset < size; offset += CHUNK) offsets.push(offset);
      const parts = new Array(offsets.length);
      let next = 0;
      let done = 0;
      const worker = async () => {
        while (next < offsets.length) {
          const k = next++;
          const out = await exec(root, chunk(offsets[k]), 60);
          if (out.code !== 0) throw new Error("Couldn't read the scan back");
          parts[k] = out.output.replace(/\s+/g, "");
          onProgress?.(++done / offsets.length);
        }
      };
      await Promise.all(Array.from({ length: Math.min(6, offsets.length) }, worker));
      const b64 = parts.join("");
      if (b64.length !== size) throw new Error("The scan came back incomplete");
      if (!size) return "";
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      return await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"))).text();
    } finally {
      void exec(root, IS_WIN ? `Remove-Item -LiteralPath ${q(file)} -ErrorAction SilentlyContinue` : `rm -f ${q(file)}`, 15).catch(() => {});
    }
  }

  /** Scans every project of a system and links them; concurrent calls share one scan. */
  function scan(id) {
    const sys = systems.find((x) => x.id === id);
    if (!sys) return Promise.resolve(null);
    const running = scans.get(id);
    if (running) return running.promise;
    const state = { startedAt: Date.now(), steps: {}, promise: null };
    scans.set(id, state);
    const step = (root, text) => {
      state.steps[root] = text;
      changed();
    };
    state.promise = (async () => {
      const projects = await Promise.all(
        sys.projects.map(async (p) => {
          try {
            step(p.root, "Listing files…");
            const dump = await readPacked(p.root, IS_WIN ? DUMP_WIN : DUMP_UNIX, (f) => step(p.root, `Reading ${Math.round(f * 100)}%`));
            step(p.root, "Analyzing…");
            await new Promise((resolve) => setTimeout(resolve, 0));
            const facts = factsFromDump(dump);
            step(p.root, "Done");
            return { root: p.root, name: p.name, facts };
          } catch (err) {
            step(p.root, "Failed");
            return { root: p.root, name: p.name, facts: null, error: String(err?.message ?? err) };
          }
        }),
      );
      const linked = linkSystem(projects);
      results.set(id, linked);
      try {
        localStorage.setItem(resultKey(id), JSON.stringify(linked));
      } catch {
        // Too big to keep across restarts; it stays for this session.
      }
      return linked;
    })().finally(() => {
      scans.delete(id);
      changed();
    });
    changed();
    return state.promise;
  }

  /** Rescans a system a little after agents in one of its projects finish. */
  function projectTouched(root) {
    if (!settings.autoRescan || !root) return;
    for (const sys of systemsFor(root)) {
      if (!result(sys.id)) continue;
      clearTimeout(timers.get(sys.id));
      timers.set(
        sys.id,
        setTimeout(() => {
          timers.delete(sys.id);
          void scan(sys.id);
        }, 30_000),
      );
    }
  }

  const norm = (p) => String(p ?? "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  const within = (a, b) => a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`);
  /** Systems with a project at, inside or around this folder. */
  function systemsFor(dir) {
    const d = norm(dir);
    if (!d) return [];
    return systems.filter((sys) => sys.projects.some((p) => within(norm(p.root), d)));
  }

  let graphifyBin = null;
  async function graphify() {
    if (!graphifyBin) graphifyBin = (await exec("", IS_WIN ? GRAPHIFY_WIN : GRAPHIFY_UNIX, 20)).output.trim().split("\n").pop()?.trim() || "";
    return graphifyBin;
  }

  return {
    settings,
    systems,
    result,
    scanning: (id) => scans.get(id) ?? null,
    scan,
    exec,
    absolute,
    graphify,
    systemsFor,
    projectTouched,
    get activeId() {
      return systems.some((x) => x.id === activeId) ? activeId : systems[0]?.id ?? null;
    },
    setActive(id) {
      activeId = id;
      save();
      changed();
    },
    /** Creates or replaces a system; rescans it when its projects changed. */
    saveSystem(sys) {
      const i = systems.findIndex((x) => x.id === sys.id);
      const before = i >= 0 ? systems[i].projects.map((p) => p.root).join("|") : null;
      if (i >= 0) systems[i] = sys;
      else systems.push(sys);
      activeId = sys.id;
      save();
      changed();
      if (before !== sys.projects.map((p) => p.root).join("|")) void scan(sys.id);
    },
    removeSystem(id) {
      const i = systems.findIndex((x) => x.id === id);
      if (i >= 0) systems.splice(i, 1);
      results.delete(id);
      try {
        localStorage.removeItem(resultKey(id));
      } catch {
        // Nothing to clean up.
      }
      if (activeId === id) activeId = systems[0]?.id ?? null;
      save();
      changed();
    },
    set(patch) {
      Object.assign(settings, patch);
      save();
      changed();
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    dispose() {
      timers.forEach((t) => clearTimeout(t));
    },
  };
}

// Agent tool text -------------------------------------------------------------------------------

function roleSummary(c, name) {
  const by = (pred) => c.roots.filter((r) => pred(c.per[r])).map(name);
  const parts = [];
  const def = by((x) => x.defines.length);
  const ser = by((x) => x.counts.ser);
  const deser = by((x) => x.counts.deser);
  const refs = by((x) => !x.defines.length && !x.counts.ser && !x.counts.deser && x.counts.refs);
  if (def.length) parts.push(`defined in ${def.join(", ")}`);
  if (ser.length) parts.push(`serialized by ${ser.join(", ")}`);
  if (deser.length) parts.push(`deserialized by ${deser.join(", ")}`);
  if (refs.length) parts.push(`used in ${refs.join(", ")}`);
  return parts.join("; ");
}

function overviewText(sys, res) {
  const names = Object.fromEntries(sys.projects.map((p) => [p.root, p.name]));
  const name = (root) => names[root] ?? baseName(root);
  const lines = [`System "${sys.name}": ${plural(sys.projects.length, "linked project")}, scanned ${ago(res.scannedAt)}.`, ""];
  for (const p of res.projects) {
    const langs = Object.entries(p.langs).map(([l, n]) => `${LANG_LABELS[l] ?? l} ${n}`).join(", ");
    lines.push(`- ${p.name}: ${p.root}${p.error ? ` (scan failed: ${p.error})` : ` (${langs || "no source files found"}; serves ${p.endpoints} endpoints, makes ${p.calls} HTTP calls)`}`);
  }
  const strong = res.contracts.filter((c) => c.strength === "strong");
  lines.push("", `Shared contracts: ${strong.length}${res.contracts.length > strong.length ? ` (+${res.contracts.length - strong.length} weaker name matches)` : ""}`);
  for (const c of strong.slice(0, 40)) lines.push(`- ${c.name}${c.package ? ` [${c.package}]` : ""}: ${roleSummary(c, name)}${c.problems ? `; ${plural(c.problems, "field")} differ` : ""}`);
  if (strong.length > 40) lines.push(`- … ${strong.length - 40} more`);
  lines.push("", `HTTP links: ${res.apis.length}`);
  for (const a of res.apis.slice(0, 30)) lines.push(`- ${a.method} ${a.path}: served by ${name(a.server.root)} (${a.server.handler || `${a.server.file}:${a.server.line}`}), called from ${[...new Set(a.clients.map((x) => name(x.root)))].join(", ")}`);
  if (res.apis.length > 30) lines.push(`- … ${res.apis.length - 30} more`);
  const pk = res.packages.filter((p) => p.contracts || p.drift || p.providers.length).slice(0, 20);
  if (pk.length) {
    lines.push("", "Shared packages:");
    for (const p of pk) lines.push(`- ${p.name} (${p.ecosystem}): ${p.roots.map((r) => `${name(r)} ${(p.uses[r] ?? []).map((u) => u.version).filter(Boolean)[0] ?? (p.providers.some((x) => x.root === r) ? "publishes it" : "?")}`).join(", ")}${p.drift ? " — versions differ" : ""}${p.contracts ? `; ${plural(p.contracts, "contract")}` : ""}`);
  }
  const problems = problemsOf(res, names);
  if (problems.length) {
    lines.push("", "Problems:");
    for (const pr of problems.slice(0, 20)) lines.push(`- ${pr.title}${pr.detail ? `: ${pr.detail}` : ""}`);
  }
  lines.push("", "xkb_contract shows one contract's fields per project and every file:line that uses it; xkb_impact what a change to it touches in the other projects.");
  return lines.join("\n");
}

function sitesText(root, label, list, total) {
  if (!total) return [];
  return [`  ${label} (${total}):`, ...list.map((x) => `    ${joinPath(root, x.file)}:${x.line}${x.via ? `  (${x.via})` : ""}`), ...(total > list.length ? [`    … ${total - list.length} more`] : [])];
}

function contractText(sys, res, c) {
  const names = Object.fromEntries(sys.projects.map((p) => [p.root, p.name]));
  const name = (root) => names[root] ?? baseName(root);
  const lines = [`${c.name}${c.package ? ` (from package ${c.package})` : ""}: ${roleSummary(c, name)}.`];
  if (c.strength === "weak") lines.push("Weak link: only the name matches, nothing shows it being serialized across projects.");
  if (c.fields) {
    lines.push("", `Fields (JSON name: type per project)${c.fields.problems ? `, ${plural(c.fields.problems, "difference")}` : ", all match"}:`);
    for (const row of c.fields.rows) {
      const cells = c.fields.roots.map((r) => (row.cells[r] ? `${name(r)} ${row.cells[r].json ? `${row.cells[r].name}→"${row.cells[r].json}"` : row.cells[r].name}: ${row.cells[r].type}` : `${name(r)}: MISSING`));
      lines.push(`- ${row.issue === "missing" ? "⚠ missing  " : row.issue === "type" ? "⚠ type     " : ""}${cells.join(" | ")}`);
    }
  } else {
    const def = c.roots.map((r) => c.per[r].defines.find((d) => d.fields.length) && r).find(Boolean);
    if (def) {
      const d = c.per[def].defines.find((x) => x.fields.length);
      lines.push("", `Fields (from ${name(def)}): ${d.fields.map((f) => `${f.name}${f.json ? `→"${f.json}"` : ""}: ${f.type}`).join(", ")}`);
    } else lines.push("", "No project has its source (it comes from a compiled package); look at the package's model to see its fields.");
  }
  for (const r of c.roots) {
    const x = c.per[r];
    lines.push("", `${name(r)} (${r}):`);
    for (const d of x.defines) lines.push(`  defined: ${joinPath(r, d.file)}:${d.line} (${d.kind}${d.namespace ? ` in ${d.namespace}` : ""})`);
    lines.push(...sitesText(r, "serialized / sent", x.ser, x.counts.ser), ...sitesText(r, "deserialized / received", x.deser, x.counts.deser), ...sitesText(r, "other uses", x.refs, x.counts.refs));
  }
  const apis = res.apis.filter((a) => c.apis.includes(a.id));
  if (apis.length) lines.push("", "Carried by:", ...apis.map((a) => `- ${a.method} ${a.path} (${name(a.server.root)} ← ${[...new Set(a.clients.map((x) => name(x.root)))].join(", ")})`));
  return lines.join("\n");
}

function apiText(sys, a) {
  const names = Object.fromEntries(sys.projects.map((p) => [p.root, p.name]));
  const name = (root) => names[root] ?? baseName(root);
  const lines = [`${a.method} ${a.path}`, `  served by ${name(a.server.root)}: ${joinPath(a.server.root, a.server.file)}:${a.server.line}${a.server.handler ? ` (${a.server.handler})` : ""}`];
  if (a.server.request.length) lines.push(`  request body: ${a.server.request.join(", ")}`);
  if (a.server.response.length) lines.push(`  response: ${a.server.response.join(", ")}`);
  for (const c of a.clients) lines.push(`  called from ${name(c.root)}: ${joinPath(c.root, c.file)}:${c.line}${c.literal ? " (path string)" : ""}${c.response.length ? ` → ${c.response.join(", ")}` : ""}${c.request.length ? ` sends ${c.request.join(", ")}` : ""}`);
  for (const i of a.issues) lines.push(`  ⚠ ${i.side}: ${i.server} on the server vs ${i.client} in ${name(i.clientRoot)}${i.fields?.problems ? ` (${plural(i.fields.problems, "field")} differ: ${i.fields.rows.filter((r) => r.issue).map((r) => r.key).join(", ")})` : ""}`);
  return lines.join("\n");
}

// Page ------------------------------------------------------------------------------------------

const LENSES = [
  ["contracts", "Contracts", "braces"],
  ["apis", "APIs", "plug"],
  ["packages", "Packages", "box"],
];
const CARD_W = 210;
const CARD_H = 96;
const PILL_H = 30;
const PILL_GAP = 8;
const GROUP_H = 30;

/** replaceChildren that skips empty slots. */
const fill = (el, ...children) => el.replaceChildren(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));

function mountPage(root, store, api) {
  const ui = { lens: "contracts", search: "", problemsOnly: false, selected: null, hover: null, editing: null };
  const style = h("style", {}, PAGE_CSS);
  const top = h("div", { class: "xp-top" });
  const status = h("div", { class: "xp-status" });
  const mapWrap = h("div", { class: "xp-map" });
  const side = h("aside", { class: "xp-side" });
  const body = h("div", { class: "xp-body" }, mapWrap, side);
  const dialogHost = h("div");
  const page = h("div", { class: "wf-page xp" }, style, top, status, body, dialogHost);
  root.append(page);

  const sys = () => store.systems.find((x) => x.id === store.activeId) ?? null;
  const res = () => (sys() ? store.result(sys().id) : null);
  const names = () => Object.fromEntries((sys()?.projects ?? []).map((p) => [p.root, p.name]));
  const nameOf = (r) => names()[r] ?? baseName(r);
  const colorOf = (r) => projectColor(Math.max(0, (sys()?.projects ?? []).findIndex((p) => p.root === r)));

  function openFile(rootDir, file, line) {
    api.execute({ type: "browser.open", url: fileUrl(joinPath(rootDir, file)), newTab: true });
    void line;
  }
  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      api.toast("Copied");
    } catch {
      api.toast("Couldn't copy to the clipboard", "error");
    }
  }

  // Header ---------------------------------------------------------------------------------------

  function renderTop() {
    const current = sys();
    const scanning = current ? store.scanning(current.id) : null;
    const select = h(
      "select",
      { class: "wf-select xp-system", onchange: (e) => (e.target.value === "__new" ? openEditor(null) : (store.setActive(e.target.value), (ui.selected = null))) },
      store.systems.map((x) => h("option", { value: x.id, selected: x.id === current?.id }, x.name)),
      h("option", { value: "__new" }, "New system…"),
    );
    fill(
      top,
      h("div", { class: "xp-title" }, h("div", { class: "xp-logo" }, icon("link")), h("div", {}, h("div", { class: "wf-h1" }, "Cross-project"), h("div", { class: "xp-sub" }, "Contracts, APIs and packages shared between projects"))),
      h("div", { class: "xp-grow" }),
      current ? select : null,
      current
        ? h(
            "div",
            { class: "xp-seg", role: "tablist" },
            LENSES.map(([id, label, ic]) => h("button", { class: `xp-seg-b${ui.lens === id ? " on" : ""}`, role: "tab", "aria-selected": ui.lens === id ? "true" : "false", onclick: () => ((ui.lens = id), (ui.selected = null), render()) }, icon(ic), label)),
          )
        : null,
      current
        ? h(
            "label",
            { class: "xp-search" },
            icon("search"),
            h("input", { placeholder: "Filter", value: ui.search, oninput: (e) => ((ui.search = e.target.value), renderMap(), renderSide()) }),
          )
        : null,
      current ? h("label", { class: "xp-toggle", title: "Show only items with problems" }, h("input", { type: "checkbox", class: "wf-switch", checked: ui.problemsOnly, onchange: (e) => ((ui.problemsOnly = e.target.checked), renderMap(), renderSide()) }), "Problems") : null,
      current ? h("button", { class: "wf-button wf-ghost", title: "Edit linked projects", onclick: () => openEditor(current) }, icon("edit"), "Projects") : null,
      current ? h("button", { class: "wf-button wf-primary", disabled: !!scanning, onclick: () => void store.scan(current.id) }, icon("refresh", `xp-i${scanning ? " xp-spin" : ""}`), scanning ? "Scanning…" : "Rescan") : null,
    );
  }

  function renderStatus() {
    const current = sys();
    const r = res();
    const scanning = current ? store.scanning(current.id) : null;
    if (!current) {
      status.replaceChildren();
      return;
    }
    if (scanning) {
      status.replaceChildren(
        ...current.projects.map((p) => h("span", { class: "xp-chip" }, h("i", { class: "xp-dot", style: `background:${colorOf(p.root)}` }), p.name, h("span", { class: "xp-muted" }, scanning.steps[p.root] ?? "Waiting…"))),
      );
      return;
    }
    if (!r) {
      status.replaceChildren(h("span", { class: "xp-muted" }, "Not scanned yet."));
      return;
    }
    const problems = problemsOf(r, names());
    status.replaceChildren(
      h("span", {}, h("b", {}, r.contracts.filter((c) => c.strength === "strong").length), " contracts"),
      h("span", {}, h("b", {}, r.apis.length), " HTTP links"),
      h("span", {}, h("b", {}, r.packages.length), " shared packages"),
      problems.length ? h("span", { class: "xp-warn" }, icon("alert"), h("b", {}, problems.length), ` ${problems.length === 1 ? "problem" : "problems"}`) : h("span", { class: "xp-ok" }, icon("check"), "No drift found"),
      h("span", { class: "xp-muted" }, `Scanned ${ago(r.scannedAt)}`),
      ...r.projects.filter((p) => p.error).map((p) => h("span", { class: "xp-err" }, `${p.name}: ${p.error}`)),
    );
  }

  // Map ------------------------------------------------------------------------------------------

  /** The middle column for the current lens: items, their groups and their edges to projects. */
  function lensData(r) {
    const term = ui.search.trim().toLowerCase();
    const match = (...texts) => !term || texts.some((t) => String(t ?? "").toLowerCase().includes(term));
    const items = [];
    if (ui.lens === "contracts") {
      for (const c of r.contracts) {
        if (c.strength === "weak" && !store.settings.showWeak && !(ui.selected?.id === c.name)) continue;
        if (ui.problemsOnly && !c.problems) continue;
        if (!match(c.name, c.package)) continue;
        const edges = c.roots.map((root) => {
          const x = c.per[root];
          const role = x.counts.ser && x.counts.deser ? "both" : x.counts.ser ? "out" : x.counts.deser ? "in" : x.defines.length ? "def" : "ref";
          return { root, role, label: "" };
        });
        const definedHere = c.roots.some((root) => c.per[root].defines.length);
        items.push({ id: c.name, label: c.name, group: c.package || (definedHere ? "Defined in these projects" : "From compiled packages"), sub: c.package ? "" : "", badge: c.problems, weak: c.strength === "weak", edges });
      }
    } else if (ui.lens === "apis") {
      for (const a of r.apis) {
        if (ui.problemsOnly && !a.issues.length) continue;
        if (!match(a.path, a.method, a.server.handler)) continue;
        const edges = [{ root: a.server.root, role: "in", label: "" }];
        for (const root of new Set(a.clients.map((c) => c.root))) edges.push({ root, role: "out", label: "" });
        items.push({ id: a.id, label: a.path, method: a.method, group: `Served by ${nameOf(a.server.root)}`, badge: a.issues.length, edges });
      }
    } else {
      for (const p of r.packages) {
        if (ui.problemsOnly && !p.drift) continue;
        if (!match(p.name)) continue;
        const max = [...p.versions].sort((a, b) => b.localeCompare(a, undefined, { numeric: true }))[0];
        const edges = p.roots.map((root) => {
          const v = cleanVersion((p.uses[root] ?? []).map((u) => u.version).find(Boolean) ?? p.providers.find((x) => x.root === root)?.version ?? "");
          return { root, role: p.providers.some((x) => x.root === root) ? "def" : "ref", label: v, warn: p.drift && v && v !== max };
        });
        items.push({ id: p.name, label: p.name, group: p.contracts ? "Carry shared contracts" : { nuget: "NuGet", npm: "npm", pub: "pub", maven: "Maven" }[p.ecosystem] ?? p.ecosystem, badge: p.drift ? 1 : 0, edges });
      }
    }
    return items;
  }

  function renderMap() {
    const current = sys();
    const r = res();
    if (!current) {
      mapWrap.replaceChildren(renderWelcome());
      return;
    }
    if (!r) {
      const scanning = store.scanning(current.id);
      mapWrap.replaceChildren(h("div", { class: "xp-empty" }, icon(scanning ? "refresh" : "link", `xp-big${scanning ? " xp-spin" : ""}`), h("div", {}, scanning ? "Reading the projects…" : "Scan the projects to see what they share."), scanning ? null : h("button", { class: "wf-button wf-primary", onclick: () => void store.scan(current.id) }, "Scan now")));
      return;
    }
    const items = lensData(r);
    const projects = current.projects;
    if (!items.length) {
      const what = { contracts: "shared contracts", apis: "HTTP calls between these projects", packages: "shared packages" }[ui.lens];
      mapWrap.replaceChildren(h("div", { class: "xp-empty" }, icon({ contracts: "braces", apis: "plug", packages: "box" }[ui.lens], "xp-big"), h("div", {}, ui.search || ui.problemsOnly ? "Nothing matches the filter." : `No ${what} found.`)));
      return;
    }

    // Servers on the right, clients on the left.
    const info = Object.fromEntries(r.projects.map((p) => [p.root, p]));
    const order = projects.map((p, i) => ({ p, i, score: (info[p.root]?.endpoints ?? 0) - (info[p.root]?.calls ?? 0) * 0.5 })).sort((a, b) => a.score - b.score || a.i - b.i);
    const leftCount = projects.length === 1 ? 1 : Math.ceil(projects.length / 2);
    const left = order.slice(0, leftCount).map((x) => x.p);
    const right = order.slice(leftCount).map((x) => x.p);

    // Groups in order, items by group.
    const groups = new Map();
    for (const it of items) {
      if (!groups.has(it.group)) groups.set(it.group, []);
      groups.get(it.group).push(it);
    }
    const W = Math.max(760, mapWrap.clientWidth || 900);
    const middleH = [...groups.values()].reduce((sum, list) => sum + GROUP_H + list.length * (PILL_H + PILL_GAP), 0) + 40;
    const H = Math.max(middleH + 50, Math.max(left.length, right.length) * (CARD_H + 36) + 60, (mapWrap.clientHeight || 500) - 4);
    const cardPos = new Map();
    const placeSide = (list, x) => list.forEach((p, i) => cardPos.set(p.root, { x, y: clamp((H * (i + 0.5)) / list.length - CARD_H / 2, 12, H - CARD_H - 12), side: x < W / 2 ? "left" : "right" }));
    placeSide(left, 20);
    placeSide(right, W - 20 - CARD_W);
    const gutter = 64;
    const pillW = Math.min(360, W - 2 * (20 + CARD_W + gutter));
    const pillX = (W - pillW) / 2;

    // Order items inside each group by where their projects sit, to untangle edges.
    const cy = (root) => (cardPos.get(root)?.y ?? 0) + CARD_H / 2;
    let y = Math.max(20, (H - middleH) / 2 + 20);
    const pills = [];
    for (const [group, list] of groups) {
      list.sort((a, b) => a.edges.reduce((t, e) => t + cy(e.root), 0) / a.edges.length - b.edges.reduce((t, e) => t + cy(e.root), 0) / b.edges.length || a.label.localeCompare(b.label));
      pills.push({ header: group, y });
      y += GROUP_H;
      for (const it of list) {
        pills.push({ it, y });
        y += PILL_H + PILL_GAP;
      }
    }

    const svg = s("svg", { class: `xp-svg${ui.hover ? " xp-focus" : ""}${store.settings.motion ? " xp-motion" : ""}`, width: W, height: H, viewBox: `0 0 ${W} ${H}` });
    svg.addEventListener("click", (e) => {
      if (e.target === svg) {
        ui.selected = null;
        renderSide();
        paintSelection();
      }
    });
    const edgeLayer = s("g", { class: "xp-edges" });
    const pillLayer = s("g");
    const cardLayer = s("g");
    svg.append(edgeLayer, pillLayer, cardLayer);

    // Edges, with anchors fanned out along each card's side.
    const anchors = new Map();
    for (const pl of pills) if (pl.it) for (const e of pl.it.edges) if (cardPos.has(e.root)) (anchors.get(e.root) ?? anchors.set(e.root, []).get(e.root)).push({ pl, e });
    for (const [rootDir, list] of anchors) {
      const c = cardPos.get(rootDir);
      list.sort((a, b) => a.pl.y - b.pl.y);
      const spread = Math.min(CARD_H - 24, list.length * 5);
      list.forEach(({ pl, e }, k) => {
        const ay = c.y + CARD_H / 2 + (list.length > 1 ? (k / (list.length - 1) - 0.5) * spread : 0);
        const ax = c.side === "left" ? c.x + CARD_W : c.x;
        const px = c.side === "left" ? pillX : pillX + pillW;
        const py = pl.y + PILL_H / 2;
        const dx = Math.max(40, Math.abs(px - ax) * 0.45) * (c.side === "left" ? 1 : -1);
        const d = `M${ax},${ay} C${ax + dx},${ay} ${px - dx},${py} ${px},${py}`;
        const color = e.warn ? "var(--color-warning)" : colorOf(rootDir);
        const toPill = e.role === "out";
        const g = s("g", { class: `xp-e xp-${e.role}`, "data-item": pl.it.id, "data-root": rootDir });
        g.append(s("path", { d, stroke: color, class: "xp-line" }));
        if (e.role === "out" || e.role === "in" || e.role === "both") {
          g.append(s("path", { d, stroke: color, class: `xp-flow ${toPill ? "xp-fwd" : e.role === "in" ? "xp-back" : "xp-fwd"}` }));
          if (e.role === "both") g.append(s("path", { d, stroke: color, class: "xp-flow xp-back xp-late" }));
          // An arrow at the receiving end.
          const [tx, ty, dir] = toPill ? [px, py, c.side === "left" ? 1 : -1] : [ax, ay, c.side === "left" ? -1 : 1];
          if (e.role !== "both") g.append(s("path", { d: `M${tx - dir * 7},${ty - 4} L${tx},${ty} L${tx - dir * 7},${ty + 4}`, class: "xp-arrow", stroke: color }));
        }
        if (e.label) {
          const lx = ax + (c.side === "left" ? 8 : -8);
          g.append(s("text", { x: lx + (px - ax) * 0.5, y: (ay + py) / 2 - 4, class: `xp-elabel${e.warn ? " xp-warn-t" : ""}`, "text-anchor": "middle" }, e.label));
        }
        edgeLayer.append(g);
      });
    }

    // Middle column.
    for (const pl of pills) {
      if (pl.header) {
        pillLayer.append(s("text", { x: pillX + 4, y: pl.y + 19, class: "xp-group" }, pl.header));
        continue;
      }
      const it = pl.it;
      const g = s("g", {
        class: `xp-pill${it.weak ? " xp-weak" : ""}${it.badge ? " xp-bad" : ""}`,
        "data-item": it.id,
        transform: `translate(${pillX},${pl.y})`,
        tabindex: 0,
        role: "button",
        onmouseenter: () => hover({ item: it.id }),
        onmouseleave: () => hover(null),
        onclick: () => select({ lens: ui.lens, id: it.id }),
        onkeydown: (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), select({ lens: ui.lens, id: it.id })),
      });
      g.append(s("rect", { width: pillW, height: PILL_H, rx: 8, class: "xp-pill-bg" }));
      let tx = 12;
      if (it.method) {
        g.append(s("text", { x: tx, y: 19.5, class: `xp-method xp-m-${it.method.toLowerCase()}` }, it.method));
        tx += 8 + it.method.length * 7.4;
      }
      const maxChars = Math.floor((pillW - tx - (it.badge ? 40 : 14)) / 7);
      g.append(s("text", { x: tx, y: 19.5, class: "xp-pill-t" }, it.label.length > maxChars ? `${it.label.slice(0, maxChars - 1)}…` : it.label));
      if (it.badge) {
        g.append(s("circle", { cx: pillW - 16, cy: PILL_H / 2, r: 9, class: "xp-badge" }));
        g.append(s("text", { x: pillW - 16, y: PILL_H / 2 + 3.8, class: "xp-badge-t", "text-anchor": "middle" }, ui.lens === "packages" ? "!" : it.badge));
      }
      g.append(s("title", {}, it.label));
      pillLayer.append(g);
    }

    // Project cards.
    for (const p of projects) {
      const c = cardPos.get(p.root);
      if (!c) continue;
      const pi = info[p.root];
      const color = colorOf(p.root);
      const langs = Object.entries(pi?.langs ?? {}).sort((a, b) => b[1] - a[1]).slice(0, 3).map(([l]) => LANG_LABELS[l] ?? l).join(" · ");
      const g = s("g", {
        class: "xp-card",
        "data-root": p.root,
        transform: `translate(${c.x},${c.y})`,
        tabindex: 0,
        role: "button",
        onmouseenter: () => hover({ root: p.root }),
        onmouseleave: () => hover(null),
        onclick: () => select({ project: p.root }),
        onkeydown: (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), select({ project: p.root })),
      });
      g.append(
        s("rect", { width: CARD_W, height: CARD_H, rx: 12, class: "xp-card-bg" }),
        s("rect", { x: c.side === "left" ? CARD_W - 4 : 0, y: 14, width: 4, height: CARD_H - 28, rx: 2, fill: color }),
        s("circle", { cx: 18, cy: 22, r: 5, fill: color }),
        s("text", { x: 30, y: 26.5, class: "xp-card-t" }, p.name.length > 22 ? `${p.name.slice(0, 21)}…` : p.name),
        s("text", { x: 14, y: 46, class: "xp-card-s" }, pi?.error ? "Scan failed" : langs || "No source found"),
        s("text", { x: 14, y: 66, class: "xp-card-s" }, pi && !pi.error ? `${plural(pi.files, "file")} · ${pi.endpoints} endpoints · ${pi.calls} calls` : ""),
        s("text", { x: 14, y: 84, class: "xp-card-p" }, p.root.length > 32 ? `…${p.root.slice(-31)}` : p.root),
        s("title", {}, p.root),
      );
      cardLayer.append(g);
    }

    fill(mapWrap, svg, legend());
    paintSelection();
  }

  function legend() {
    const row = (cls, text) => h("span", { class: "xp-leg" }, h("i", { class: `xp-leg-l ${cls}` }), text);
    const rows =
      ui.lens === "contracts"
        ? [row("xp-l-out", "serializes / sends"), row("xp-l-in", "deserializes / receives"), row("xp-l-def", "defines"), row("xp-l-ref", "mentions")]
        : ui.lens === "apis"
          ? [row("xp-l-out", "calls"), row("xp-l-in", "serves")]
          : [row("xp-l-def", "publishes"), row("xp-l-ref", "uses (version on the line)")];
    return h(
      "div",
      { class: "xp-legend" },
      rows,
      ui.lens === "contracts" ? h("label", { class: "xp-leg xp-leg-t" }, h("input", { type: "checkbox", checked: store.settings.showWeak, onchange: (e) => store.set({ showWeak: e.target.checked }) }), "name-only matches") : null,
    );
  }

  function hover(target) {
    ui.hover = target;
    paintSelection();
  }

  /** Highlights the hovered or selected item's edges and dims the rest. */
  function paintSelection() {
    const svg = mapWrap.querySelector("svg");
    if (!svg) return;
    const focus = ui.hover ?? (ui.selected?.id !== undefined && ui.selected?.lens === ui.lens ? { item: ui.selected.id } : ui.selected?.project ? { root: ui.selected.project } : null);
    svg.classList.toggle("xp-focus", !!focus);
    for (const el of svg.querySelectorAll(".xp-e, .xp-pill, .xp-card")) {
      const on = !!focus && ((focus.item !== undefined && el.dataset.item === focus.item) || (focus.root && el.dataset.root === focus.root) || (focus.root && el.classList.contains("xp-pill") && svg.querySelector(`.xp-e[data-item="${CSS.escape(el.dataset.item)}"][data-root="${CSS.escape(focus.root)}"]`)) || (focus.item !== undefined && el.classList.contains("xp-card") && svg.querySelector(`.xp-e[data-item="${CSS.escape(focus.item)}"][data-root="${CSS.escape(el.dataset.root)}"]`)));
      el.classList.toggle("on", !!on);
    }
    for (const el of svg.querySelectorAll(".xp-pill")) el.classList.toggle("sel", ui.selected?.lens === ui.lens && el.dataset.item === ui.selected?.id);
  }

  function select(target) {
    ui.selected = target;
    if (target?.lens && target.lens !== ui.lens) {
      ui.lens = target.lens;
      renderTop();
      renderMap();
    }
    renderSide();
    paintSelection();
  }

  // Side panel -----------------------------------------------------------------------------------

  const siteRow = (rootDir, x, extra) =>
    h(
      "div",
      { class: "xp-site" },
      h("button", { class: "xp-link wf-mono", title: `Open ${joinPath(rootDir, x.file)}`, onclick: () => openFile(rootDir, x.file, x.line) }, `${x.file}:${x.line}`),
      extra ? h("span", { class: "xp-muted" }, extra) : null,
      h("button", { class: "xp-icon-b", title: "Copy path", onclick: () => copy(`${joinPath(rootDir, x.file)}:${x.line}`) }, icon("copy")),
    );
  const section = (title, ...children) => h("div", { class: "xp-sec" }, h("div", { class: "xp-sec-h" }, title), ...children);
  const projectTag = (rootDir) => h("button", { class: "xp-ptag", onclick: () => select({ project: rootDir }) }, h("i", { class: "xp-dot", style: `background:${colorOf(rootDir)}` }), nameOf(rootDir));
  const back = () => h("button", { class: "xp-back", onclick: () => select(null) }, icon("arrowLeft"), "Overview");

  function renderSide() {
    const r = res();
    const current = sys();
    if (!current || !r) {
      fill(side, current ? h("div", { class: "xp-side-empty" }, "Results show up here after a scan.") : renderHow());
      return;
    }
    const sel = ui.selected;
    if (sel?.lens === "contracts") {
      const c = r.contracts.find((x) => x.name === sel.id);
      if (c) return side.replaceChildren(renderContract(r, c));
    } else if (sel?.lens === "apis") {
      const a = r.apis.find((x) => x.id === sel.id);
      if (a) return side.replaceChildren(renderApi(a));
    } else if (sel?.lens === "packages") {
      const p = r.packages.find((x) => x.name === sel.id);
      if (p) return side.replaceChildren(renderPackage(r, p));
    } else if (sel?.project) {
      const p = r.projects.find((x) => x.root === sel.project);
      if (p) return side.replaceChildren(renderProject(r, p));
    }
    side.replaceChildren(renderOverview(r));
  }

  function renderOverview(r) {
    const problems = problemsOf(r, names());
    return h(
      "div",
      { class: "xp-panel" },
      h("div", { class: "wf-h2" }, sys().name),
      h("div", { class: "xp-muted xp-small" }, `${plural(r.projects.length, "project")} · scanned ${ago(r.scannedAt)}`),
      section(
        "Projects",
        ...r.projects.map((p) =>
          h(
            "button",
            { class: "xp-prow", onclick: () => select({ project: p.root }) },
            h("i", { class: "xp-dot", style: `background:${colorOf(p.root)}` }),
            h("div", { class: "xp-grow" }, h("div", { class: "xp-prow-t" }, p.name), h("div", { class: "xp-muted xp-small wf-mono" }, p.root)),
            p.error ? h("span", { class: "wf-badge xp-b-err" }, "failed") : h("span", { class: "xp-muted xp-small" }, plural(p.files, "file")),
          ),
        ),
      ),
      section(
        problems.length ? `Problems (${problems.length})` : "Problems",
        problems.length
          ? problems.slice(0, 60).map((pr) =>
              h(
                "button",
                { class: `xp-prob xp-k-${pr.kind}`, onclick: () => select(pr.select) },
                icon(pr.kind === "unmatched" ? "plug" : pr.kind === "package" ? "box" : "alert"),
                h("div", { class: "xp-grow" }, h("div", {}, pr.title), pr.detail ? h("div", { class: "xp-muted xp-small" }, pr.detail) : null),
              ),
            )
          : h("div", { class: "xp-okbox" }, icon("check"), "Every shared contract, endpoint and package lines up."),
      ),
      h("div", { class: "xp-hint" }, "Agents in these projects can call ", h("code", {}, "xkb_overview"), ", ", h("code", {}, "xkb_contract"), " and ", h("code", {}, "xkb_impact"), " to see the same links before they change a model."),
    );
  }

  function renderContract(r, c) {
    const roles = [];
    for (const rootDir of c.roots) {
      const x = c.per[rootDir];
      const what = [x.defines.length && "defines", x.counts.ser && "serializes", x.counts.deser && "deserializes", !x.defines.length && !x.counts.ser && !x.counts.deser && "mentions"].filter(Boolean).join(", ");
      roles.push(h("div", { class: "xp-role" }, projectTag(rootDir), h("span", { class: "xp-muted" }, what)));
    }
    const fieldTable = c.fields
      ? h(
          "table",
          { class: "xp-table" },
          h("thead", {}, h("tr", {}, h("th", {}, "JSON"), ...c.fields.roots.map((rt) => h("th", {}, h("i", { class: "xp-dot", style: `background:${colorOf(rt)}` }), nameOf(rt))))),
          h(
            "tbody",
            {},
            c.fields.rows.map((row) =>
              h(
                "tr",
                { class: row.issue ? `xp-row-${row.issue}` : "" },
                h("td", { class: "wf-mono" }, Object.values(row.cells)[0].json || Object.values(row.cells)[0].name),
                ...c.fields.roots.map((rt) => (row.cells[rt] ? h("td", { class: "wf-mono", title: row.cells[rt].name }, row.cells[rt].type) : h("td", { class: "xp-missing" }, "missing"))),
              ),
            ),
          ),
        )
      : null;
    const single = !c.fields ? c.roots.map((rt) => ({ rt, d: c.per[rt].defines.find((d) => d.fields.length) })).find((x) => x.d) : null;
    const apis = r.apis.filter((a) => c.apis.includes(a.id));
    return h(
      "div",
      { class: "xp-panel" },
      back(),
      h("div", { class: "xp-head" }, h("div", { class: "wf-h2 wf-mono" }, c.name), c.package ? h("button", { class: "wf-badge wf-accent", onclick: () => select({ lens: "packages", id: c.package }) }, icon("box"), c.package) : null, c.strength === "weak" ? h("span", { class: "wf-badge" }, "name match") : null),
      h("div", { class: "xp-roles" }, roles),
      c.fields
        ? section(c.fields.problems ? `Fields · ${plural(c.fields.problems, "difference")}` : "Fields · all match", fieldTable)
        : single
          ? section(`Fields (${nameOf(single.rt)})`, h("div", { class: "xp-fields" }, single.d.fields.map((f) => h("span", { class: "xp-field wf-mono" }, f.json || f.name, h("span", { class: "xp-muted" }, `: ${f.type}`)))))
          : section("Fields", h("div", { class: "xp-muted xp-small" }, "No project has this model's source; it comes from a compiled package.")),
      ...c.roots.map((rt) => {
        const x = c.per[rt];
        const list = (label, sites, total) => (total ? [h("div", { class: "xp-sub-h" }, `${label} · ${total}`), ...sites.map((site) => siteRow(rt, site, site.via)), total > sites.length ? h("div", { class: "xp-muted xp-small" }, `+${total - sites.length} more`) : null] : []);
        return section(
          h("span", { class: "xp-inline" }, h("i", { class: "xp-dot", style: `background:${colorOf(rt)}` }), nameOf(rt)),
          ...x.defines.map((d) => siteRow(rt, d, `${d.kind}${d.namespace ? ` · ${d.namespace}` : ""}`)),
          ...list("Serialized", x.ser, x.counts.ser),
          ...list("Deserialized", x.deser, x.counts.deser),
          ...list("Mentioned", x.refs, x.counts.refs),
        );
      }),
      apis.length ? section("Carried by", ...apis.map((a) => h("button", { class: "xp-api-row", onclick: () => select({ lens: "apis", id: a.id }) }, h("span", { class: `xp-method xp-m-${a.method.toLowerCase()}` }, a.method), h("span", { class: "wf-mono" }, a.path)))) : null,
      h(
        "div",
        { class: "xp-actions" },
        h("button", { class: "wf-button", onclick: () => copy(contractText(sys(), r, c)) }, icon("copy"), "Copy summary"),
        h("button", { class: "wf-button wf-primary", onclick: () => askAgent(c, r) }, icon("bot"), c.problems ? "Fix with an agent" : "Ask an agent"),
      ),
    );
  }

  function askAgent(c, r) {
    const current = sys();
    const s0 = api.state();
    const target = c.roots.find((rt) => c.fields?.rows.some((row) => row.missing.includes(rt))) ?? c.roots[0];
    const ws = s0.workspaces.find((w) => store.systemsFor(w.rootDir).some((x) => x.id === current.id) && String(w.rootDir).replace(/[\\/]+$/, "") === target.replace(/[\\/]+$/, "")) ?? s0.workspaces.find((w) => w.id === s0.activeWorkspaceId);
    const prompt = [
      c.problems ? `The data contract ${c.name} has drifted between linked projects. Make every side agree with the source of truth${c.package ? ` (the ${c.package} package)` : ""}, keeping the JSON on the wire compatible.` : `Explain how the data contract ${c.name} is used across these linked projects.`,
      "Details from winfer's cross-project scan (call xkb_contract or xkb_impact for more):",
      contractText(current, r, c),
    ].join("\n\n");
    if (ws && ws.id !== s0.activeWorkspaceId) api.execute({ type: "workspace.switch", target: { id: ws.id } });
    api.execute({ type: "pane.spawn", provider: "default", prompt });
    api.execute({ type: "ui.view", view: "workspace" });
  }

  function renderApi(a) {
    return h(
      "div",
      { class: "xp-panel" },
      back(),
      h("div", { class: "xp-head" }, h("span", { class: `xp-method xp-m-${a.method.toLowerCase()}` }, a.method), h("div", { class: "wf-h2 wf-mono" }, a.path)),
      section(
        h("span", { class: "xp-inline" }, "Served by ", projectTag(a.server.root)),
        siteRow(a.server.root, a.server, a.server.handler),
        a.server.request.length ? h("div", { class: "xp-kv" }, h("span", { class: "xp-muted" }, "Request"), typeLinks(a.server.request)) : null,
        a.server.response.length ? h("div", { class: "xp-kv" }, h("span", { class: "xp-muted" }, "Response"), typeLinks(a.server.response)) : null,
      ),
      section(
        `Called from (${a.clients.length})`,
        ...a.clients.map((c) => h("div", { class: "xp-client" }, projectTag(c.root), siteRow(c.root, c, c.literal ? "path string" : c.response.length ? `→ ${c.response.join(", ")}` : ""))),
      ),
      a.issues.length
        ? section(
            "Mismatches",
            ...a.issues.map((i) =>
              h(
                "div",
                { class: "xp-issue" },
                icon("alert"),
                h(
                  "div",
                  {},
                  h("div", {}, `The ${i.side} is `, h("b", {}, i.server), ` on the server but `, h("b", {}, i.client), ` in ${nameOf(i.clientRoot)}.`),
                  i.fields ? h("div", { class: "xp-muted xp-small" }, i.fields.problems ? `Fields that differ: ${i.fields.rows.filter((row) => row.issue).map((row) => row.key).join(", ")}` : "Their fields match.") : null,
                ),
              ),
            ),
          )
        : null,
    );
  }

  function typeLinks(list) {
    const r = res();
    return h("span", { class: "xp-types" }, list.map((t) => (r.contracts.some((c) => c.name === t) ? h("button", { class: "xp-link wf-mono", onclick: () => select({ lens: "contracts", id: t }) }, t) : h("span", { class: "wf-mono" }, t))));
  }

  function renderPackage(r, p) {
    const contracts = r.contracts.filter((c) => c.package === p.name);
    return h(
      "div",
      { class: "xp-panel" },
      back(),
      h("div", { class: "xp-head" }, h("div", { class: "wf-h2 wf-mono" }, p.name), h("span", { class: "wf-badge" }, p.ecosystem), p.drift ? h("span", { class: "wf-badge xp-b-warn" }, "versions differ") : null),
      p.providers.length ? section("Published by", ...p.providers.map((pr) => h("div", { class: "xp-client" }, projectTag(pr.root), siteRow(pr.root, { file: pr.file, line: 1 }, pr.version ? `v${pr.version}` : "")))) : null,
      section(
        "Used by",
        ...Object.entries(p.uses).flatMap(([rt, list]) => list.map((u) => h("div", { class: "xp-client" }, projectTag(rt), siteRow(rt, { file: u.file, line: 1 }, u.version || "no version"))))),
      contracts.length ? section(`Contracts (${contracts.length})`, h("div", { class: "xp-fields" }, contracts.map((c) => h("button", { class: `xp-field wf-mono${c.problems ? " xp-field-bad" : ""}`, onclick: () => select({ lens: "contracts", id: c.name }) }, c.name)))) : null,
    );
  }

  function renderProject(r, p) {
    const contracts = r.contracts.filter((c) => c.roots.includes(p.root) && (c.strength === "strong" || store.settings.showWeak));
    const serves = r.apis.filter((a) => a.server.root === p.root);
    const calls = r.apis.filter((a) => a.clients.some((c) => c.root === p.root));
    return h(
      "div",
      { class: "xp-panel" },
      back(),
      h("div", { class: "xp-head" }, h("i", { class: "xp-dot xp-dot-l", style: `background:${colorOf(p.root)}` }), h("div", { class: "wf-h2" }, p.name)),
      h("div", { class: "xp-muted xp-small wf-mono" }, p.root),
      p.error ? h("div", { class: "wf-error" }, p.error) : h("div", { class: "xp-fields" }, Object.entries(p.langs).map(([l, n]) => h("span", { class: "xp-field" }, `${LANG_LABELS[l] ?? l} · ${n}`))),
      section(`Contracts (${contracts.length})`, h("div", { class: "xp-fields" }, contracts.map((c) => h("button", { class: `xp-field wf-mono${c.problems ? " xp-field-bad" : ""}`, onclick: () => select({ lens: "contracts", id: c.name }) }, c.name)))),
      serves.length ? section(`Serves (${serves.length})`, ...serves.slice(0, 40).map((a) => h("button", { class: "xp-api-row", onclick: () => select({ lens: "apis", id: a.id }) }, h("span", { class: `xp-method xp-m-${a.method.toLowerCase()}` }, a.method), h("span", { class: "wf-mono" }, a.path)))) : null,
      calls.length ? section(`Calls (${calls.length})`, ...calls.slice(0, 40).map((a) => h("button", { class: "xp-api-row", onclick: () => select({ lens: "apis", id: a.id }) }, h("span", { class: `xp-method xp-m-${a.method.toLowerCase()}` }, a.method), h("span", { class: "wf-mono" }, a.path), h("span", { class: "xp-muted" }, `→ ${nameOf(a.server.root)}`)))) : null,
    );
  }

  function renderHow() {
    return h(
      "div",
      { class: "xp-panel" },
      h("div", { class: "wf-h2" }, "How it works"),
      h("ol", { class: "xp-steps" }, h("li", {}, "Link the projects that talk to each other: a mobile app, a web API, the package with their shared models."), h("li", {}, "winfer reads their source (C#, TypeScript, Kotlin, Java, Swift, Dart, Go, Rust, Zig), manifests and OpenAPI documents, including generated Swagger clients (NSwag, Kiota, Refit)."), h("li", {}, "You see which models travel between them, which endpoints each one calls, which packages they share — and where they drifted apart.")),
    );
  }

  function renderWelcome() {
    return h(
      "div",
      { class: "xp-welcome" },
      h("div", { class: "xp-hero" }, s("svg", { viewBox: "0 0 240 90", class: "xp-hero-art", "aria-hidden": "true" }, s("rect", { x: 4, y: 25, width: 62, height: 40, rx: 9, class: "xp-art-card" }), s("rect", { x: 174, y: 25, width: 62, height: 40, rx: 9, class: "xp-art-card" }), s("rect", { x: 94, y: 33, width: 52, height: 24, rx: 7, class: "xp-art-pill" }), s("path", { d: "M66 45 C80 45 80 45 94 45", class: "xp-art-line" }), s("path", { d: "M146 45 C160 45 160 45 174 45", class: "xp-art-line" }), s("text", { x: 120, y: 49, "text-anchor": "middle", class: "xp-art-t" }, "{ }"))),
      h("div", { class: "wf-h1" }, "Link projects that talk to each other"),
      h("div", { class: "xp-muted" }, "See the models one project serializes and another deserializes, the endpoints between them and the packages they share, and catch drift before it breaks the app."),
      h("button", { class: "wf-button wf-primary", onclick: () => openEditor(null) }, icon("plus"), "Link projects"),
    );
  }

  // System editor --------------------------------------------------------------------------------

  function openEditor(existing) {
    const draft = existing ? { ...existing, projects: existing.projects.map((p) => ({ ...p })) } : { id: `sys-${Date.now().toString(36)}`, name: "", projects: [] };
    let pathInput = "";
    let error = "";
    let busy = false;
    const close = () => dialogHost.replaceChildren();
    const workspaceRoots = () => [...new Map(api.state().workspaces.filter((w) => w.rootDir).map((w) => [w.rootDir, w])).values()];
    async function addPath(raw) {
      const dir = String(raw ?? "").trim().replace(/[\\/]+$/, "");
      if (!dir) return;
      busy = true;
      error = "";
      draw();
      try {
        const abs = await store.absolute(dir);
        const r = await store.exec(abs, IS_WIN ? "(Get-Location).Path" : "pwd -P", 15);
        if (r.code !== 0) throw new Error("not found");
        const real = r.output.trim().split("\n").pop().trim() || abs;
        if (draft.projects.some((p) => p.root === abs || p.root === real)) throw new Error("already linked");
        draft.projects.push({ root: abs, name: baseName(abs) });
        pathInput = "";
      } catch (err) {
        error = `Can't add ${dir}: ${String(err?.message ?? err)}`;
      } finally {
        busy = false;
        draw();
      }
    }
    function draw() {
      const free = workspaceRoots().filter((w) => !draft.projects.some((p) => p.root === w.rootDir));
      const saveIt = () => {
        if (draft.projects.length < 2) {
          error = "Link at least two projects.";
          draw();
          return;
        }
        store.saveSystem({ ...draft, name: draft.name.trim() || draft.projects.map((p) => p.name).join(" + ") });
        ui.selected = null;
        close();
      };
      dialogHost.replaceChildren(
        h(
          "div",
          { class: "wf-dialog-backdrop", onclick: (e) => e.target === e.currentTarget && close() },
          h(
            "div",
            { class: "wf-dialog xp-dialog", role: "dialog", "aria-label": "Linked projects", onkeydown: (e) => e.key === "Escape" && close() },
            h("div", { class: "wf-h2" }, existing ? "Linked projects" : "Link projects"),
            h("label", { class: "wf-field" }, "Name", h("input", { class: "wf-input", placeholder: "e.g. Shop (mobile + web)", value: draft.name, oninput: (e) => (draft.name = e.target.value) })),
            h(
              "div",
              { class: "xp-plist" },
              draft.projects.length
                ? draft.projects.map((p, i) =>
                    h(
                      "div",
                      { class: "xp-pedit" },
                      h("i", { class: "xp-dot", style: `background:${projectColor(i)}` }),
                      h("input", { class: "wf-input xp-pname", value: p.name, "aria-label": "Project name", oninput: (e) => (p.name = e.target.value || baseName(p.root)) }),
                      h("span", { class: "xp-muted xp-small wf-mono xp-grow xp-ell", title: p.root }, p.root),
                      h("button", { class: "xp-icon-b", title: "Remove", onclick: () => (draft.projects.splice(i, 1), draw()) }, icon("x")),
                    ),
                  )
                : h("div", { class: "xp-muted xp-small" }, "No projects yet."),
            ),
            free.length
              ? h(
                  "div",
                  { class: "xp-add" },
                  h("div", { class: "xp-sub-h" }, "Open workspaces"),
                  h("div", { class: "xp-fields" }, free.map((w) => h("button", { class: "xp-field", title: w.rootDir, onclick: () => void addPath(w.rootDir) }, icon("plus"), w.name || baseName(w.rootDir)))),
                )
              : null,
            h(
              "div",
              { class: "xp-add" },
              h("div", { class: "xp-sub-h" }, "Any folder"),
              h(
                "div",
                { class: "xp-inline" },
                h("input", { class: "wf-input xp-grow", placeholder: IS_WIN ? "C:\\code\\mobile-app" : "~/code/mobile-app", value: pathInput, oninput: (e) => (pathInput = e.target.value), onkeydown: (e) => e.key === "Enter" && void addPath(pathInput) }),
                h("button", { class: "wf-button", disabled: busy, onclick: () => void addPath(pathInput) }, icon("folder"), "Add"),
              ),
            ),
            error ? h("div", { class: "wf-error" }, error) : null,
            h(
              "div",
              { class: "xp-dialog-foot" },
              existing ? h("button", { class: "wf-button wf-danger wf-ghost", onclick: async () => (await api.confirm(`Remove the system "${existing.name}"? The projects themselves aren't touched.`, { title: "Remove system", okLabel: "Remove", kind: "danger" })) && (store.removeSystem(existing.id), close()) }, "Remove system") : null,
              h("div", { class: "xp-grow" }),
              h("button", { class: "wf-button wf-ghost", onclick: close }, "Cancel"),
              h("button", { class: "wf-button wf-primary", disabled: busy, onclick: saveIt }, existing ? "Save" : "Link and scan"),
            ),
          ),
        ),
      );
      dialogHost.querySelector(".wf-dialog input")?.focus();
    }
    draw();
  }

  // Wiring ---------------------------------------------------------------------------------------

  function render() {
    renderTop();
    renderStatus();
    renderMap();
    renderSide();
  }
  let lastScanning = null;
  const unsub = store.subscribe(() => {
    const scanning = sys() ? !!store.scanning(sys().id) : false;
    renderTop();
    renderStatus();
    // While scanning, only the status line changes.
    if (!scanning || lastScanning !== scanning) {
      renderMap();
      renderSide();
    }
    lastScanning = scanning;
  });
  let width = 0;
  const resize = new ResizeObserver(() => {
    if (Math.abs(mapWrap.clientWidth - width) < 24) return;
    width = mapWrap.clientWidth;
    renderMap();
  });
  resize.observe(mapWrap);
  const theme = new MutationObserver(() => renderMap());
  theme.observe(document.documentElement, { attributes: true, attributeFilter: ["data-mode"] });
  render();
  const current = sys();
  if (current && !store.result(current.id) && !store.scanning(current.id)) void store.scan(current.id);
  return () => {
    unsub();
    resize.disconnect();
    theme.disconnect();
  };
}

const PAGE_CSS = `
.xp { max-width: none; padding: 20px 24px 24px; gap: 12px; height: 100%; box-sizing: border-box; display: flex; flex-direction: column; container-type: inline-size; }
.xp-i { width: 15px; height: 15px; flex-shrink: 0; }
.xp-big { width: 34px; height: 34px; color: var(--color-fg-subtle); }
.xp-spin { animation: xp-spin 1s linear infinite; }
@keyframes xp-spin { to { transform: rotate(360deg); } }
.xp-top { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.xp-title { display: flex; align-items: center; gap: 12px; }
.xp-logo { width: 34px; height: 34px; border-radius: 10px; display: flex; align-items: center; justify-content: center; background: var(--color-accent-soft); color: var(--color-accent); }
.xp-logo .xp-i { width: 18px; height: 18px; }
.xp-sub { font-size: 12px; color: var(--color-fg-muted); margin-top: 2px; }
.xp-grow { flex: 1; min-width: 0; }
.xp-system { max-width: 220px; }
.xp-seg { display: inline-flex; padding: 2px; border-radius: 9px; background: var(--color-panel-raised); border: 1px solid var(--color-border); }
.xp-seg-b { display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 10px; border: 0; border-radius: 7px; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 12px; cursor: pointer; }
.xp-seg-b:hover { color: var(--color-fg); }
.xp-seg-b.on { background: var(--color-panel); color: var(--color-fg); box-shadow: 0 1px 2px rgb(0 0 0 / .12); }
.xp-seg-b:focus-visible, .xp-link:focus-visible, .xp-prow:focus-visible, .xp-prob:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 1px; }
.xp-search { display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); color: var(--color-fg-subtle); }
.xp-search:focus-within { border-color: var(--color-accent); }
.xp-search input { width: 130px; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 12px; }
.xp-toggle { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--color-fg-muted); }
.xp-status { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; min-height: 22px; font-size: 12px; color: var(--color-fg-muted); }
.xp-status b { color: var(--color-fg); font-weight: 600; font-variant-numeric: tabular-nums; }
.xp-status > span { display: inline-flex; align-items: center; gap: 5px; }
.xp-warn { color: var(--color-warning); }
.xp-warn b { color: var(--color-warning); }
.xp-ok { color: var(--color-success); }
.xp-err { color: var(--color-danger); }
.xp-muted { color: var(--color-fg-muted); }
.xp-small { font-size: 11px; }
.xp-chip { display: inline-flex; align-items: center; gap: 6px; padding: 2px 10px 2px 8px; border-radius: 999px; background: var(--color-panel-raised); border: 1px solid var(--color-border); color: var(--color-fg); }
.xp-dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
.xp-dot-l { width: 11px; height: 11px; }
.xp-body { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) 380px; gap: 14px; }
@container (max-width: 1100px) { .xp { height: auto; min-height: 100%; } .xp-body { flex: none; grid-template-columns: minmax(0, 1fr); grid-template-rows: 560px auto; } .xp-side { overflow: visible; } }
.xp-map { position: relative; min-height: 0; overflow: auto; border-radius: 14px; border: 1px solid var(--color-border); background:
  radial-gradient(circle at 1px 1px, color-mix(in srgb, var(--color-fg) 9%, transparent) 1px, transparent 0) 0 0 / 22px 22px, var(--color-panel); }
.xp-svg { display: block; }
.xp-line { fill: none; stroke-width: 1.6; opacity: .55; transition: opacity .15s; }
.xp-def .xp-line { stroke-dasharray: 6 4; }
.xp-ref .xp-line { stroke-dasharray: 1.5 4; stroke-width: 1.4; opacity: .4; }
.xp-flow { fill: none; stroke-width: 2.4; stroke-linecap: round; stroke-dasharray: 2 16; opacity: 0; }
.xp-motion .xp-flow { opacity: .9; animation: xp-flow 1.6s linear infinite; }
.xp-motion .xp-flow.xp-back { animation-direction: reverse; }
.xp-motion .xp-flow.xp-late { animation-delay: -.8s; }
@keyframes xp-flow { to { stroke-dashoffset: -18; } }
.xp-arrow { fill: none; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
.xp-elabel { font-size: 10.5px; fill: var(--color-fg-muted); font-family: var(--font-mono); paint-order: stroke; stroke: var(--color-panel); stroke-width: 4px; }
.xp-warn-t { fill: var(--color-warning); font-weight: 600; }
.xp-e { transition: opacity .15s; }
.xp-focus .xp-e:not(.on) { opacity: .07; }
.xp-focus .xp-e.on .xp-line { opacity: 1; stroke-width: 2.2; }
.xp-focus .xp-pill:not(.on):not(.sel), .xp-focus .xp-card:not(.on) { opacity: .35; }
.xp-pill, .xp-card { cursor: pointer; outline: none; transition: opacity .15s; }
.xp-pill-bg { fill: var(--color-panel-raised); stroke: var(--color-border-strong); stroke-width: 1; transition: stroke .15s, fill .15s; }
.xp-pill:hover .xp-pill-bg, .xp-pill:focus-visible .xp-pill-bg, .xp-pill.on .xp-pill-bg { stroke: var(--color-accent); }
.xp-pill.sel .xp-pill-bg { stroke: var(--color-accent); stroke-width: 2; fill: color-mix(in srgb, var(--color-accent) 10%, var(--color-panel-raised)); }
.xp-pill.xp-weak .xp-pill-bg { stroke-dasharray: 3 3; fill: transparent; }
.xp-pill.xp-bad .xp-pill-bg { stroke: color-mix(in srgb, var(--color-warning) 70%, var(--color-border-strong)); }
.xp-pill-t { font-size: 12.5px; fill: var(--color-fg); font-family: var(--font-mono); }
.xp-weak .xp-pill-t { fill: var(--color-fg-muted); }
.xp-badge { fill: var(--color-warning); }
.xp-badge-t { font-size: 10.5px; font-weight: 700; fill: var(--color-app, #000); }
.xp-group { font-size: 10.5px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; fill: var(--color-fg-subtle); }
.xp-method { font-family: var(--font-mono); font-size: 10.5px; font-weight: 700; letter-spacing: .02em; fill: var(--color-accent); color: var(--color-accent); }
.xp-m-get { fill: var(--color-success); color: var(--color-success); }
.xp-m-delete { fill: var(--color-danger); color: var(--color-danger); }
.xp-m-put, .xp-m-patch { fill: var(--color-warning); color: var(--color-warning); }
.xp-card-bg { fill: var(--color-panel-raised); stroke: var(--color-border-strong); transition: stroke .15s; filter: drop-shadow(0 2px 6px rgb(0 0 0 / .12)); }
.xp-card:hover .xp-card-bg, .xp-card:focus-visible .xp-card-bg, .xp-card.on .xp-card-bg { stroke: var(--color-accent); }
.xp-card-t { font-size: 13.5px; font-weight: 600; fill: var(--color-fg); }
.xp-card-s { font-size: 11.5px; fill: var(--color-fg-muted); }
.xp-card-p { font-size: 10.5px; fill: var(--color-fg-subtle); font-family: var(--font-mono); }
.xp-legend { position: sticky; left: 10px; bottom: 10px; display: inline-flex; gap: 14px; flex-wrap: wrap; margin: -46px 0 10px 10px; padding: 7px 12px; border-radius: 9px; background: color-mix(in srgb, var(--color-panel) 88%, transparent); backdrop-filter: blur(8px); border: 1px solid var(--color-border); font-size: 11px; color: var(--color-fg-muted); }
.xp-leg { display: inline-flex; align-items: center; gap: 6px; }
.xp-leg-t input { margin: 0; }
.xp-leg-l { display: inline-block; width: 22px; height: 0; border-top: 2px solid var(--color-fg-muted); }
.xp-l-out { border-top-style: solid; position: relative; }
.xp-l-out::after { content: "›"; position: absolute; right: -3px; top: -9px; font-size: 13px; }
.xp-l-in { border-top-style: solid; position: relative; }
.xp-l-in::after { content: "‹"; position: absolute; left: -3px; top: -9px; font-size: 13px; }
.xp-l-def { border-top-style: dashed; }
.xp-l-ref { border-top-style: dotted; }
.xp-empty { height: 100%; min-height: 300px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px; color: var(--color-fg-muted); font-size: 13px; text-align: center; padding: 24px; }
.xp-welcome { height: 100%; min-height: 380px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px; text-align: center; padding: 32px; max-width: 520px; margin: 0 auto; line-height: 1.5; }
.xp-hero-art { width: 240px; height: 90px; margin-bottom: 6px; }
.xp-art-card { fill: var(--color-panel-raised); stroke: var(--color-border-strong); }
.xp-art-pill { fill: var(--color-accent-soft); stroke: var(--color-accent); }
.xp-art-line { fill: none; stroke: var(--color-accent); stroke-width: 2; stroke-dasharray: 2 7; stroke-linecap: round; }
.xp-motion .xp-art-line, .xp-welcome .xp-art-line { animation: xp-flow 1.4s linear infinite; }
.xp-art-t { font-size: 12px; fill: var(--color-accent); font-family: var(--font-mono); font-weight: 700; }
.xp-side { min-height: 0; overflow-y: auto; border-radius: 14px; border: 1px solid var(--color-border); background: var(--color-panel); }
.xp-side-empty { padding: 24px; color: var(--color-fg-muted); font-size: 12px; }
.xp-panel { display: flex; flex-direction: column; gap: 12px; padding: 16px 16px 20px; font-size: 12.5px; color: var(--color-fg); }
.xp-head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.xp-head .wf-h2 { overflow-wrap: anywhere; }
.xp-head .wf-badge { display: inline-flex; align-items: center; gap: 4px; border: 0; cursor: default; font: inherit; font-size: 11px; }
.xp-head button.wf-badge { cursor: pointer; }
.xp-b-warn { background: color-mix(in srgb, var(--color-warning) 18%, transparent); color: var(--color-warning); }
.xp-b-err { background: color-mix(in srgb, var(--color-danger) 16%, transparent); color: var(--color-danger); }
.xp-back { align-self: flex-start; display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 8px 0 4px; border: 0; border-radius: 6px; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 11.5px; cursor: pointer; }
.xp-back:hover { background: var(--color-hover); color: var(--color-fg); }
.xp-sec { display: flex; flex-direction: column; gap: 4px; padding-top: 10px; border-top: 1px solid var(--color-border); }
.xp-sec-h { font-size: 11px; font-weight: 600; letter-spacing: .05em; text-transform: uppercase; color: var(--color-fg-subtle); margin-bottom: 4px; }
.xp-sub-h { font-size: 11px; color: var(--color-fg-muted); margin-top: 6px; }
.xp-inline { display: inline-flex; align-items: center; gap: 6px; }
.xp-roles { display: flex; flex-direction: column; gap: 4px; }
.xp-role { display: flex; align-items: center; gap: 8px; }
.xp-ptag { display: inline-flex; align-items: center; gap: 6px; padding: 2px 9px 2px 7px; border-radius: 999px; border: 1px solid var(--color-border); background: var(--color-panel-raised); color: var(--color-fg); font: inherit; font-size: 11.5px; cursor: pointer; text-transform: none; letter-spacing: 0; font-weight: 500; }
.xp-ptag:hover { border-color: var(--color-border-strong); }
.xp-site { display: flex; align-items: center; gap: 8px; min-height: 24px; }
.xp-site .xp-muted { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 11px; }
.xp-link { border: 0; padding: 0; background: none; color: var(--color-accent); font-size: 11.5px; cursor: pointer; text-align: left; overflow-wrap: anywhere; }
.xp-link:hover { text-decoration: underline; }
.xp-icon-b { display: inline-flex; align-items: center; justify-content: center; width: 22px; height: 22px; flex-shrink: 0; border: 0; border-radius: 5px; background: transparent; color: var(--color-fg-subtle); cursor: pointer; opacity: .7; }
.xp-icon-b .xp-i { width: 13px; height: 13px; }
.xp-icon-b:hover { background: var(--color-hover); color: var(--color-fg); opacity: 1; }
.xp-table { width: 100%; border-collapse: collapse; font-size: 11.5px; }
.xp-table th { text-align: left; font-weight: 600; color: var(--color-fg-muted); padding: 4px 6px; border-bottom: 1px solid var(--color-border); white-space: nowrap; }
.xp-table th .xp-dot { margin-right: 5px; }
.xp-table td { padding: 4px 6px; border-bottom: 1px solid color-mix(in srgb, var(--color-border) 60%, transparent); overflow-wrap: anywhere; }
.xp-row-missing td, .xp-row-type td { background: color-mix(in srgb, var(--color-warning) 9%, transparent); }
.xp-missing { color: var(--color-danger); font-style: italic; }
.xp-fields { display: flex; flex-wrap: wrap; gap: 5px; }
.xp-field { display: inline-flex; align-items: center; gap: 4px; padding: 2px 8px; border-radius: 6px; background: var(--color-panel-raised); border: 1px solid var(--color-border); font-size: 11.5px; color: var(--color-fg); font-family: inherit; }
button.xp-field { cursor: pointer; }
button.xp-field:hover { border-color: var(--color-accent); }
.xp-field .xp-i { width: 12px; height: 12px; }
.xp-field-bad { border-color: color-mix(in srgb, var(--color-warning) 60%, transparent); }
.xp-prow { display: flex; align-items: center; gap: 10px; padding: 7px 8px; margin: 0 -8px; border: 0; border-radius: 8px; background: transparent; text-align: left; color: inherit; font: inherit; cursor: pointer; }
.xp-prow:hover { background: var(--color-hover); }
.xp-prow-t { font-weight: 600; }
.xp-prow .wf-mono { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.xp-prob { display: flex; gap: 9px; padding: 7px 8px; margin: 0 -8px; border: 0; border-radius: 8px; background: transparent; text-align: left; color: inherit; font: inherit; line-height: 1.45; cursor: pointer; }
.xp-prob:hover { background: var(--color-hover); }
.xp-prob > .xp-i { margin-top: 2px; color: var(--color-warning); }
.xp-k-unmatched > .xp-i { color: var(--color-fg-subtle); }
.xp-okbox, .xp-issue { display: flex; gap: 8px; align-items: flex-start; padding: 9px 10px; border-radius: 8px; line-height: 1.45; }
.xp-okbox { background: color-mix(in srgb, var(--color-success) 10%, transparent); color: var(--color-success); }
.xp-issue { background: color-mix(in srgb, var(--color-warning) 10%, transparent); }
.xp-issue > .xp-i { color: var(--color-warning); margin-top: 1px; }
.xp-kv { display: flex; gap: 8px; align-items: baseline; }
.xp-types { display: inline-flex; gap: 6px; flex-wrap: wrap; }
.xp-client { display: flex; flex-direction: column; gap: 2px; padding: 4px 0; }
.xp-client .xp-ptag { align-self: flex-start; }
.xp-api-row { display: flex; align-items: baseline; gap: 8px; padding: 4px 6px; margin: 0 -6px; border: 0; border-radius: 6px; background: transparent; color: var(--color-fg); font: inherit; font-size: 11.5px; text-align: left; cursor: pointer; }
.xp-api-row:hover { background: var(--color-hover); }
.xp-actions { display: flex; gap: 8px; flex-wrap: wrap; padding-top: 6px; }
.xp-hint { font-size: 11.5px; color: var(--color-fg-subtle); line-height: 1.5; }
.xp-hint code { font-family: var(--font-mono); color: var(--color-fg-muted); }
.xp-steps { margin: 0; padding-left: 18px; display: flex; flex-direction: column; gap: 8px; color: var(--color-fg-muted); line-height: 1.5; }
.xp-dialog { width: min(620px, calc(100vw - 32px)); display: flex; flex-direction: column; gap: 12px; }
.xp-plist { display: flex; flex-direction: column; gap: 6px; padding: 8px; border-radius: 10px; border: 1px solid var(--color-border); background: var(--color-panel-raised); max-height: 260px; overflow-y: auto; }
.xp-pedit { display: flex; align-items: center; gap: 8px; }
.xp-pname { width: 150px; flex-shrink: 0; }
.xp-ell { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.xp-add { display: flex; flex-direction: column; gap: 6px; }
.xp-dialog-foot { display: flex; align-items: center; gap: 8px; padding-top: 4px; }
@media (prefers-reduced-motion: reduce) { .xp-motion .xp-flow, .xp-art-line, .xp-spin { animation: none; } }
`;

// Activation ------------------------------------------------------------------------------------

export function activate(api) {
  const store = createStore(api);
  const text = (v) => String(v ?? "").trim();
  const rootOf = (workspaceId) => api.state().workspaces.find((w) => w.id === workspaceId)?.rootDir;

  /** The caller's system (by name, else the one its project is linked into) and a fresh enough scan. */
  async function context(caller, wanted, { maxAgeMs = 30 * 60_000 } = {}) {
    let sys = null;
    if (text(wanted)) sys = store.systems.find((x) => x.name.toLowerCase() === text(wanted).toLowerCase() || x.id === text(wanted));
    if (!sys) sys = store.systemsFor(caller.cwd)[0] ?? store.systemsFor(rootOf(caller.workspaceId))[0] ?? null;
    if (!sys) {
      const all = store.systems.map((x) => `"${x.name}" (${x.projects.map((p) => p.name).join(", ")})`).join("; ");
      throw new Error(
        all
          ? `This project isn't linked to other projects. Linked systems: ${all}. Pass system to pick one, or ask the user to add this project on winfer's Cross-project page.`
          : "No projects are linked yet. Ask the user to link this project with the projects it talks to (for example the mobile app and the web API) on winfer's Cross-project page.",
      );
    }
    let res = store.result(sys.id);
    if (!res || Date.now() - res.scannedAt > maxAgeMs || store.scanning(sys.id)) res = await store.scan(sys.id);
    return { sys, res };
  }
  const systemParam = { system: { type: "string", description: "Name of a linked system; default: the one the caller's project belongs to" } };
  const findContract = (res, name) => {
    const n = text(name).toLowerCase();
    return res.contracts.find((c) => c.name.toLowerCase() === n) ?? res.contracts.find((c) => c.name.toLowerCase().includes(n));
  };

  api.registerAgentTool({
    name: "xkb_overview",
    description:
      "Cross-project knowledge: the projects linked with this one (e.g. a mobile app, a web API and a shared NuGet/npm model package), the data contracts they share (who serializes and who deserializes each model), the HTTP endpoints one serves and another calls, shared package versions, and any drift between them. Call it before changing anything that crosses a project boundary.",
    inputSchema: { type: "object", properties: { ...systemParam } },
    run: async (args, caller) => {
      const { sys, res } = await context(caller, args.system);
      return truncate(overviewText(sys, res), TOOL_OUTPUT_MAX);
    },
  });

  api.registerAgentTool({
    name: "xkb_contract",
    description:
      "One shared data contract (a model/DTO sent as JSON between linked projects): its JSON fields and types in every project that defines it (with mismatches marked), the package it comes from, and every file:line that serializes, deserializes or uses it in each project, plus the endpoints that carry it.",
    inputSchema: { type: "object", properties: { name: { type: "string", description: "The model's type name, e.g. OrderDto" }, ...systemParam }, required: ["name"] },
    run: async (args, caller) => {
      const { sys, res } = await context(caller, args.system);
      const c = findContract(res, args.name);
      if (!c) throw new Error(`No shared contract named ${text(args.name)}. Shared contracts: ${res.contracts.map((x) => x.name).slice(0, 60).join(", ") || "none"}.`);
      return truncate(contractText(sys, res, c), TOOL_OUTPUT_MAX);
    },
  });

  api.registerAgentTool({
    name: "xkb_impact",
    description:
      "What changing a shared contract (or one field of it) touches in the OTHER linked projects: the files and lines that read or write it there, the endpoints that carry it, and with `field`, every line in those files that mentions the field. Use it before renaming, removing or retyping a model property.",
    inputSchema: {
      type: "object",
      properties: { name: { type: "string", description: "The model's type name" }, field: { type: "string", description: "A property/JSON field name, to find each line that uses it" }, ...systemParam },
      required: ["name"],
    },
    run: async (args, caller) => {
      const { sys, res } = await context(caller, args.system);
      const c = findContract(res, args.name);
      if (!c) throw new Error(`No shared contract named ${text(args.name)}.`);
      const names = Object.fromEntries(sys.projects.map((p) => [p.root, p.name]));
      const here = store.systemsFor(caller.cwd).length ? c.roots.find((r) => caller.cwd && String(caller.cwd).replace(/\\/g, "/").toLowerCase().startsWith(r.replace(/\\/g, "/").toLowerCase())) : null;
      const field = text(args.field);
      const variants = new Set();
      if (field) {
        variants.add(field);
        for (const x of Object.values(c.per)) for (const d of x.defines) for (const f of d.fields) if (normKey(f.name) === normKey(field) || normKey(f.json) === normKey(field)) [f.name, f.json].filter(Boolean).forEach((v) => variants.add(v));
      }
      const lines = [`Changing ${c.name}${field ? `.${field}` : ""}${here ? ` in ${names[here]}` : ""} affects:`];
      for (const r of c.roots) {
        if (r === here) continue;
        const x = c.per[r];
        lines.push("", `${names[r] ?? baseName(r)} (${r}):`);
        for (const d of x.defines) lines.push(`  own copy of the model: ${joinPath(r, d.file)}:${d.line} — keep it in sync`);
        lines.push(...sitesText(r, "writes it (serializes / sends)", x.ser, x.counts.ser), ...sitesText(r, "reads it (deserializes / receives)", x.deser, x.counts.deser), ...sitesText(r, "other uses", x.refs, x.counts.refs));
        if (field) {
          const files = [...new Set([...x.defines, ...x.ser, ...x.deser, ...x.refs].map((y) => y.file))].slice(0, 80);
          if (!files.length) continue;
          const words = [...variants].map((v) => v.replace(/[^\w$-]/g, "")).filter(Boolean);
          const cmd = IS_WIN
            ? `Select-String -LiteralPath ${files.map(q).join(",")} -Pattern ${q(`\\b(${words.join("|")})\\b`)} -CaseSensitive | Select-Object -First 80 | ForEach-Object { "$($_.Path.Substring((Get-Location).Path.Length + 1)):$($_.LineNumber):$($_.Line.Trim())" }`
            : `grep -HnwE ${q(words.join("|"))} -- ${files.map(q).join(" ")} 2>/dev/null | head -n 80`;
          try {
            const out = (await store.exec(r, cmd, 60)).output.trim();
            lines.push(`  lines mentioning ${words.join(" / ")}:`, ...(out ? out.split("\n").map((l) => `    ${joinPath(r, l.replace(/\\/g, "/"))}`) : ["    (none in the files that use the model)"]));
          } catch (err) {
            lines.push(`  (couldn't search: ${String(err?.message ?? err)})`);
          }
        }
      }
      const apis = res.apis.filter((a) => c.apis.includes(a.id));
      if (apis.length) lines.push("", "Endpoints that carry it:", ...apis.map((a) => `- ${a.method} ${a.path} (${names[a.server.root] ?? ""} ← ${[...new Set(a.clients.map((y) => names[y.root] ?? ""))].join(", ")})`));
      if (c.package) lines.push("", `It comes from the package ${c.package}: changing it means a new package version that every project above must update to.`);
      return truncate(lines.join("\n"), TOOL_OUTPUT_MAX);
    },
  });

  api.registerAgentTool({
    name: "xkb_endpoint",
    description:
      "Find HTTP endpoints between the linked projects by path or handler (e.g. 'orders' or 'POST /api/orders'): which project serves it (file:line, request/response types) and every place another project calls it, with type mismatches.",
    inputSchema: { type: "object", properties: { query: { type: "string", description: "Part of a path, optionally with a method" }, ...systemParam }, required: ["query"] },
    run: async (args, caller) => {
      const { sys, res } = await context(caller, args.system);
      const raw = text(args.query);
      const method = /^(GET|POST|PUT|PATCH|DELETE)\s+/i.exec(raw)?.[1]?.toUpperCase();
      const needle = raw.replace(/^(GET|POST|PUT|PATCH|DELETE)\s+/i, "").toLowerCase();
      const segs = normPath(needle);
      const hits = res.apis.filter((a) => (!method || a.method === method) && (a.path.includes(needle) || a.server.handler.toLowerCase().includes(needle) || (segs.length && pathScore(normPath(a.path), segs) > 0)));
      if (!hits.length) throw new Error(`No endpoint between the linked projects matches "${raw}". There are ${res.apis.length}: ${res.apis.slice(0, 30).map((a) => `${a.method} ${a.path}`).join(", ")}.`);
      return truncate(hits.slice(0, 20).map((a) => apiText(sys, a)).join("\n\n"), TOOL_OUTPUT_MAX);
    },
  });

  api.registerAgentTool({
    name: "xkb_query",
    description:
      "Ask a question across the knowledge graphs (graphify) of every linked project at once, e.g. 'how is an order created' spanning the mobile app and the web API. Returns each project's most relevant nodes with file:line. Projects without a graph are listed so you can ask the user to build one on the Knowledge page.",
    inputSchema: { type: "object", properties: { question: { type: "string" }, budget: { type: "number", description: "Max tokens per project (default 800)" }, ...systemParam }, required: ["question"] },
    run: async (args, caller) => {
      const { sys } = await context(caller, args.system, { maxAgeMs: Infinity });
      const question = text(args.question);
      if (!question) throw new Error("question is required.");
      const bin = await store.graphify();
      if (!bin) throw new Error("graphify isn't installed. Ask the user to install it from winfer's Knowledge page, then build each project's graph.");
      const budget = clamp(Math.round(Number(args.budget) || 800), 200, 3000);
      const run = `${IS_WIN ? "& " : ""}${q(bin)} query ${q(question)} --budget ${budget}`;
      const parts = await Promise.all(
        sys.projects.map(async (p) => {
          const cmd = IS_WIN ? `if (Test-Path 'graphify-out/graph.json') { ${run} } else { '@@NOGRAPH' }` : `if [ -f graphify-out/graph.json ]; then ${run} 2>&1 | head -c 6000; else echo @@NOGRAPH; fi`;
          try {
            const out = (await store.exec(p.root, cmd, 90)).output.trim();
            return out.includes("@@NOGRAPH") ? `## ${p.name} (${p.root})\nNo knowledge graph yet.` : `## ${p.name} (${p.root})\n${out}`;
          } catch (err) {
            return `## ${p.name}\n(query failed: ${String(err?.message ?? err)})`;
          }
        }),
      );
      return truncate(parts.join("\n\n"), TOOL_OUTPUT_MAX);
    },
  });

  api.registerAgentTool({
    name: "xkb_rescan",
    description: "Rescan the linked projects now (after you changed a shared model, an endpoint or a package version) and get the updated overview with any remaining drift.",
    inputSchema: { type: "object", properties: { ...systemParam } },
    run: async (args, caller) => {
      const { sys } = await context(caller, args.system, { maxAgeMs: Infinity });
      const before = new Set(problemsOf(store.result(sys.id)).map((p) => p.title));
      const res = await store.scan(sys.id);
      const after = problemsOf(res);
      const fixed = [...before].filter((t) => !after.some((p) => p.title === t));
      const fresh = after.filter((p) => !before.has(p.title));
      const head = [fixed.length ? `Resolved since the last scan: ${fixed.join("; ")}` : "", fresh.length ? `New since the last scan: ${fresh.map((p) => p.title).join("; ")}` : ""].filter(Boolean).join("\n");
      return truncate(`${head ? `${head}\n\n` : ""}${overviewText(sys, res)}`, TOOL_OUTPUT_MAX);
    },
  });

  api.registerPage({ id: "map", title: "Cross-project", icon: "waypoints", mount: (el) => mountPage(el, store, api) });

  api.registerTool({ id: "open", title: "Cross-project: open the map", description: "Show the contracts and APIs shared between linked projects", palette: true, run: () => api.openPage("map") });
  api.registerTool({
    id: "rescan",
    title: "Cross-project: rescan linked projects",
    description: "Rescan the linked projects for shared contracts, endpoints and drift",
    palette: true,
    run: () => {
      const root = rootOf(api.state().activeWorkspaceId);
      const sys = store.systemsFor(root)[0] ?? store.systems.find((x) => x.id === store.activeId);
      if (!sys) return api.openPage("map");
      void store.scan(sys.id).then((res) => res && api.toast(`${sys.name}: ${problemsOf(res).length ? plural(problemsOf(res).length, "problem") : "no drift"} across ${plural(res.contracts.length, "contract")}`, problemsOf(res).length ? "info" : "success"));
    },
  });

  api.on("pane.status", (e) => {
    if (e.previous === "working" && e.status !== "working") store.projectTouched(rootOf(e.workspaceId));
  });

  return () => store.dispose();
}
