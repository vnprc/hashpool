# Stats Dashboard Implementation Guide

**Note:** This guide documents the legacy stats-pool/stats-proxy snapshot stack. Hashpool now uses Prometheus/VictoriaMetrics with `/metrics` endpoints on pool/translator, and web dashboards query the metrics store directly. Refer to `docs/PATH_A_STATS_REPLACEMENT_PLAN.md` and current web-* code for the new flow.

Complete specification for building independent stats dashboard UIs - one for Translator (web-proxy) and one for Pool (web-pool).

---

## Table of Contents

1. [Overview](#overview)
2. [Service Overview](#service-overview)
3. [API Endpoints](#api-endpoints)
4. [Component Architecture](#component-architecture)
5. [State Management](#state-management)
6. [UI Components Reference](#ui-components-reference)
7. [Data Types & Units](#data-types--units)
8. [Implementation Patterns](#implementation-patterns)
9. [Performance Optimization](#performance-optimization)
10. [Error Handling & Recovery](#error-handling--recovery)
11. [Testing Strategy](#testing-strategy)

---

## Overview

There are **two completely independent** stats dashboards, each deployed with its own web service:

1. **web-proxy (Translator Dashboard)**
   - Deployed alongside the translator service
   - Displays miner stats from stats-proxy
   - Shows hashrate of connected miners
   - Cannot access pool data

2. **web-pool (Pool Dashboard)**
   - Deployed alongside the pool service
   - Displays translator/pool stats from stats-pool
   - Shows hashrate of connected translators and aggregate pool hashrate
   - Cannot access translator/miner data

**Key principle:** The two services are completely isolated. They do not share a web interface, cannot access each other's data, and operate independently.

---

## Service Overview

### Translator Dashboard (web-proxy)

**Purpose:** Monitor mining operation from translator perspective.

**Data source:** `stats-proxy` service (port 8084)

**Displays:**
- Connected miners and their addresses
- Per-miner hashrate (real-time and historical)
- Aggregate hashrate across all miners
- Current wallet balance (eHash)
- Upstream pool connection status
- Blockchain network (testnet4, mainnet, etc.)

**Architecture:**
```
┌─────────────────┐
│   Translator    │
│   (mining app)  │
└────────┬────────┘
         │ sends TranslatorStatus
         │ + ServiceSnapshot
         ▼
┌─────────────────────┐
│  stats-proxy        │
│  (TCP port 8082)    │
│  (HTTP port 8084)   │
│  • TranslatorStatus │
│  • Metrics (SQLite) │
└────────┬────────────┘
         │ HTTP /api/stats, /api/hashrate
         ▼
┌─────────────────────┐
│   web-proxy         │
│   (dashboard UI)    │
│   Shows miners      │
└─────────────────────┘
```

### Pool Dashboard (web-pool)

**Purpose:** Monitor mining operation from pool perspective.

**Data source:** `stats-pool` service (port 9084)

**Displays:**
- Connected translators and their addresses
- Per-translator hashrate (real-time and historical)
- Aggregate pool hashrate
- Connected services (Pool, Mint, JobDeclarator)
- Channel information
- Shares and quotes metrics

**Architecture:**
```
┌─────────────────┐
│   Pool          │
│   (mining app)  │
└────────┬────────┘
         │ sends PoolStatus
         │ + ServiceSnapshot
         ▼
┌──────────────────────┐
│  stats-pool          │
│  (TCP port 9083)     │
│  (HTTP port 9084)    │
│  • PoolStatus        │
│  • Metrics (SQLite)  │
└────────┬─────────────┘
         │ HTTP /api/stats, /api/hashrate
         ▼
┌──────────────────────┐
│   web-pool           │
│   (dashboard UI)     │
│   Shows translators  │
└──────────────────────┘
```

---

## API Endpoints

### Translator Dashboard API (stats-proxy, port 8084)

All endpoints are simple REST calls. No authentication. CORS may need to be configured in web-proxy service.

#### 1. Get Current Status

```
GET /api/stats
```

Returns current translator status with connected miners.

**Response:**
```json
{
  "ehash_balance": 500000,
  "upstream_pool": {
    "address": "stratum.example.com:3333"
  },
  "downstream_miners": [
    {
      "name": "miner_1",
      "id": 1,
      "address": "192.168.1.100:4444",
      "hashrate": 150000000000.0,
      "shares_submitted": 42,
      "connected_at": 1730488000
    },
    {
      "name": "miner_2",
      "id": 2,
      "address": "192.168.1.101:4444",
      "hashrate": 155000000000.0,
      "shares_submitted": 45,
      "connected_at": 1730488100
    }
  ],
  "blockchain_network": "testnet4",
  "timestamp": 1730488920
}
```

#### 2. Get Aggregate Hashrate

```
GET /api/hashrate?from=<unix_timestamp>&to=<unix_timestamp>
```

Get combined hashrate across all miners in time range.

**Query Parameters:**
- `from` (required): Unix timestamp (seconds)
- `to` (required): Unix timestamp (seconds)

**Response:**
```json
{
  "data": [
    {
      "timestamp": 1730488800,
      "hashrate_hs": 1500000000000.0
    },
    {
      "timestamp": 1730488810,
      "hashrate_hs": 1505000000000.0
    }
  ]
}
```

#### 3. Get Per-Miner Hashrate

```
GET /api/downstream/{miner_id}/hashrate?from=<unix_timestamp>&to=<unix_timestamp>
```

Get hashrate for a specific miner.

**Path Parameters:**
- `miner_id`: The miner's ID from /api/stats

**Response:** Same format as aggregate hashrate.

#### 4. Health Check

```
GET /health
```

Check service health.

**Response:**
```json
{
  "healthy": true,
  "stale": false
}
```

---

### Pool Dashboard API (stats-pool, port 9084)

**Identical endpoints**, but returns pool-specific data.

#### 1. Get Current Status

```
GET /api/stats
```

Returns current pool status with connected translators.

**Response:**
```json
{
  "services": [
    {
      "service_type": "Pool",
      "address": "0.0.0.0:34254"
    },
    {
      "service_type": "Mint",
      "address": "127.0.0.1:34260"
    },
    {
      "service_type": "JobDeclarator",
      "address": "127.0.0.1:34264"
    }
  ],
  "downstream_proxies": [
    {
      "id": 1,
      "address": "127.0.0.1:33866",
      "channels": [10, 11, 12],
      "shares_submitted": 260,
      "quotes_created": 130,
      "ehash_mined": 492544,
      "last_share_at": 1730488875,
      "work_selection": false
    },
    {
      "id": 2,
      "address": "127.0.0.1:33870",
      "channels": [],
      "shares_submitted": 0,
      "quotes_created": 0,
      "ehash_mined": 0,
      "last_share_at": null,
      "work_selection": true
    }
  ],
  "listen_address": "0.0.0.0:34254",
  "timestamp": 1730488875
}
```

#### 2. Get Aggregate Hashrate

```
GET /api/hashrate?from=<unix_timestamp>&to=<unix_timestamp>
```

Get combined pool hashrate across all translators.

#### 3. Get Per-Translator Hashrate

```
GET /api/downstream/{translator_id}/hashrate?from=<unix_timestamp>&to=<unix_timestamp>
```

Get hashrate for a specific translator.

#### 4. Health Check

```
GET /health
```

Check service health.

---

## Component Architecture

Both dashboards use **identical component structure** - only the data source differs.

### Common Components

```
HashrateChartPage (Parent container)
├─ Controls/Toolbar
│  ├─ Time Range Selector (1h/24h/7d/custom)
│  ├─ Refresh Button
│  └─ Export Button
├─ AggregateHashrateChart
│  └─ Line chart with real-time updates
├─ DownstreamList (Miners or Translators)
│  ├─ Sortable table/list
│  └─ Click to view per-item chart
├─ DownstreamChart (Tabbed/Modal)
│  └─ Per-item line chart with statistics
└─ HealthStatus
   └─ Connection indicator
```

### Configuration per Service

**web-proxy:**
```typescript
const config = {
  serviceUrl: 'http://localhost:8084',
  serviceName: 'Translator',
  downstreamType: 'Miners',
  downstreamField: 'downstream_miners',
};
```

**web-pool:**
```typescript
const config = {
  serviceUrl: 'http://localhost:9084',
  serviceName: 'Pool',
  downstreamType: 'Translators',
  downstreamField: 'downstream_proxies',
};
```

---

## State Management

Both services use the same Redux/Vuex structure, configured with different API endpoints:

```
store/
├── config.ts (stores serviceUrl, serviceName, downstreamType)
├── hashrate/
│   ├── state.ts
│   │   ├── aggregate: HashratePoint[]
│   │   ├── downstreams: {[id: number]: HashratePoint[]}
│   │   ├── status: 'connected' | 'stale' | 'error'
│   │   ├── timeRange: {from: number, to: number}
│   │   └── lastUpdate: number
│   │
│   ├── mutations.ts
│   │   ├── setAggregateHashrate(state, data)
│   │   ├── setDownstreamHashrate(state, {id, data})
│   │   ├── setStatus(state, status)
│   │   └── setTimeRange(state, {from, to})
│   │
│   ├── actions.ts
│   │   ├── fetchAggregateHashrate({commit}, {from, to})
│   │   ├── fetchDownstreamHashrate({commit}, {id, from, to})
│   │   ├── fetchStatus({commit})
│   │   ├── startPolling({dispatch}, interval)
│   │   └── stopPolling({commit})
│   │
│   └── getters.ts
│       ├── formattedAggregateData: () => {labels, data}
│       ├── downstreamStats: (id) => {min, max, avg}
│       └── isHealthy: () => boolean
│
└── downstreams/
    ├── state.ts
    │   ├── list: DownstreamInfo[]
    │   └── lastSnapshot: number
    │
    └── mutations.ts
        └── setDownstreams(state, list)
```

---

## UI Components Reference

### AggregateHashrateChart

Displays aggregate hashrate line chart.

**Props:**
```typescript
interface Props {
  serviceUrl: string,           // e.g., "http://localhost:8084"
  data: HashratePoint[],
  timeRange: {from: number, to: number},
  refreshInterval?: number,     // ms (default: 10000)
}
```

**Example (web-proxy):**
```typescript
<AggregateHashrateChart
  serviceUrl="http://localhost:8084"
  data={aggregateData}
  timeRange={{from: Date.now()/1000 - 3600, to: Date.now()/1000}}
  refreshInterval={10000}
/>
```

**Example (web-pool):**
```typescript
<AggregateHashrateChart
  serviceUrl="http://localhost:9084"
  data={aggregateData}
  timeRange={{from: Date.now()/1000 - 3600, to: Date.now()/1000}}
  refreshInterval={10000}
/>
```

---

### DownstreamList

Displays table/list of miners (web-proxy) or translators (web-pool).

**Props:**
```typescript
interface Props {
  serviceUrl: string,
  downstreams: DownstreamInfo[],
  downstreamType: 'Miners' | 'Translators',  // For labels
  onSelectDownstream: (id: number) => void,
}
```

**Example (web-proxy - miners):**
```typescript
<DownstreamList
  serviceUrl="http://localhost:8084"
  downstreams={miners}
  downstreamType="Miners"
  onSelectDownstream={(id) => setSelectedMinerId(id)}
/>
```

**Example (web-pool - translators):**
```typescript
<DownstreamList
  serviceUrl="http://localhost:9084"
  downstreams={translators}
  downstreamType="Translators"
  onSelectDownstream={(id) => setSelectedTranslatorId(id)}
/>
```

---

### DownstreamChart

Displays per-miner or per-translator hashrate chart.

**Props:**
```typescript
interface Props {
  serviceUrl: string,
  downstreamId: number,
  name: string,
  itemType: 'Miner' | 'Translator',  // For labels
  timeRange: {from: number, to: number},
  onClose: () => void,
}
```

---

### TimeRangeSelector

Time range picker - identical for both services.

```typescript
<TimeRangeSelector
  current={{from: Date.now()/1000 - 3600, to: Date.now()/1000}}
  onRangeChange={(from, to) => setTimeRange({from, to})}
/>
```

---

### HealthStatus

Service health indicator.

```typescript
<HealthStatus
  serviceUrl="http://localhost:8084"  // or 9084 for pool
  refreshInterval={5000}
/>
```

---

## Data Types & Units

### Hashrate

- **Unit**: Hashes per second (H/s)
- **Type**: Float64
- **Common magnitudes:**
  - 1 MH/s = 1,000,000 H/s
  - 1 GH/s = 1,000,000,000 H/s
  - 1 TH/s = 1,000,000,000,000 H/s

### Timestamps

- **Unit**: Unix seconds (not milliseconds)
- **Type**: u64
- **Data interval**: 10 seconds (one per mining window)

### HashratePoint

```typescript
interface HashratePoint {
  timestamp: number,      // Unix seconds
  hashrate_hs: number,    // Hashes per second
}
```

### DownstreamInfo (Miner - web-proxy)

```typescript
interface MinerInfo {
  name: string,
  id: u32,
  address: string,
  hashrate: f64,
  shares_submitted: u64,
  connected_at: u64,
}
```

### DownstreamInfo (Translator - web-pool)

```typescript
interface TranslatorInfo {
  id: u32,
  address: string,
  channels: u32[],
  shares_submitted: u64,
  quotes_created: u64,
  ehash_mined: u64,
  last_share_at: u64 | null,
  work_selection: bool,
}
```

---

## Implementation Patterns

### Hashrate Display Formatting

```typescript
function formatHashrate(hs: number): string {
  const units = ['H/s', 'KH/s', 'MH/s', 'GH/s', 'TH/s', 'PH/s'];
  let value = hs;
  let unitIndex = 0;

  while (value >= 1000 && unitIndex < units.length - 1) {
    value /= 1000;
    unitIndex++;
  }

  return `${value.toFixed(2)} ${units[unitIndex]}`;
}
```

### Timestamp Formatting

```typescript
function formatTimestamp(unixSeconds: number, relative: boolean = false): string {
  const date = new Date(unixSeconds * 1000);

  if (relative) {
    const now = Date.now() / 1000;
    const diff = now - unixSeconds;

    if (diff < 60) return `${Math.floor(diff)}s ago`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return `${Math.floor(diff / 86400)}d ago`;
  }

  return date.toLocaleString();
}
```

### API Fetch Pattern (Generic)

```typescript
async function fetchHashrateData(
  serviceUrl: string,
  hours: number = 24
) {
  try {
    const now = Math.floor(Date.now() / 1000);
    const from = now - (hours * 3600);

    const response = await fetch(
      `${serviceUrl}/api/hashrate?from=${from}&to=${now}`
    );

    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } catch (error) {
    console.error(`Failed to fetch from ${serviceUrl}:`, error);
    throw error;
  }
}
```

### Polling Hook (React)

```typescript
function useHashratePolling(
  serviceUrl: string,
  intervalMs: number = 10000
) {
  const [data, setData] = useState<HashratePoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const fetchData = async () => {
      try {
        const result = await fetchHashrateData(serviceUrl, 24);
        setData(result.data);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Unknown error');
      } finally {
        setLoading(false);
      }
    };

    fetchData();
    const interval = setInterval(fetchData, intervalMs);
    return () => clearInterval(interval);
  }, [serviceUrl, intervalMs]);

  return {data, loading, error};
}
```

---

## Performance Optimization

### Query Time Range Guidance

```
Last hour:      360 points (OK)
Last 24 hours:  8,640 points (OK)
Last 7 days:    60,480 points (CAUTION - may be slow)
Last 30 days:   259,200 points (NOT RECOMMENDED)
```

For large ranges, aggregate data (hourly/daily averages):

```typescript
function aggregateByHour(points: HashratePoint[]): HashratePoint[] {
  const hourly = new Map<number, HashratePoint[]>();

  for (const point of points) {
    const hour = Math.floor(point.timestamp / 3600) * 3600;
    if (!hourly.has(hour)) hourly.set(hour, []);
    hourly.get(hour)!.push(point);
  }

  return Array.from(hourly.entries()).map(([hour, pts]) => ({
    timestamp: hour,
    hashrate_hs: pts.reduce((a, b) => a + b.hashrate_hs, 0) / pts.length
  }));
}
```

### Recommended Refresh Intervals

```typescript
const refreshIntervals = {
  realtimeDashboard: 10000,   // 10 seconds (matches data interval)
  historicalView: 30000,      // 30 seconds
  backgroundMonitor: 60000    // 1 minute
};
```

### Caching Strategy

```typescript
class HashrateCache {
  private cache = new Map<string, {data: any, timestamp: number}>();
  private ttl = 5000; // 5 second TTL

  set(key: string, data: any) {
    this.cache.set(key, {data, timestamp: Date.now()});
  }

  get(key: string) {
    const entry = this.cache.get(key);
    if (!entry) return null;
    if (Date.now() - entry.timestamp > this.ttl) {
      this.cache.delete(key);
      return null;
    }
    return entry.data;
  }
}
```

### Virtualization for Large Lists

Use `react-window` or `react-virtual` to only render visible items in the DownstreamList.

---

## Error Handling & Recovery

### HTTP Status Codes

| Status | Meaning | Action |
|--------|---------|--------|
| 200 | Success | Parse response |
| 400 | Bad Request | Check parameters |
| 404 | Not Found | ID doesn't exist |
| 503 | Service Unavailable | Service is stale |
| 5xx | Server Error | Retry with backoff |

### Retry Strategy

```typescript
async function fetchWithRetry(
  url: string,
  maxRetries: number = 3,
  backoffMs: number = 1000
) {
  for (let i = 0; i < maxRetries; i++) {
    try {
      const response = await fetch(url);
      if (response.ok) return response.json();
      if (response.status >= 500) throw new Error(`HTTP ${response.status}`);
      return response.json();
    } catch (error) {
      if (i === maxRetries - 1) throw error;
      await new Promise(r => setTimeout(r, backoffMs * Math.pow(2, i)));
    }
  }
}
```

### Graceful Degradation

```typescript
function renderChart(data, error) {
  if (error) return <ErrorBoundary message={error} />;
  if (!data || data.length === 0) return <EmptyState />;
  return <LineChart data={data} />;
}
```

---

## Testing Strategy

### Unit Tests

```typescript
describe('formatHashrate', () => {
  it('formats large values correctly', () => {
    expect(formatHashrate(1500000000000)).toBe('1.50 TH/s');
    expect(formatHashrate(150000000000)).toBe('150.00 GH/s');
  });
});
```

### Component Tests

```typescript
describe('AggregateHashrateChart', () => {
  it('renders with data', async () => {
    const {getByRole} = render(
      <AggregateHashrateChart
        serviceUrl="http://localhost:8084"
        data={[{timestamp: 1000, hashrate_hs: 1500000000000}]}
        timeRange={{from: 0, to: 2000}}
      />
    );
    expect(getByRole('img', {hidden: true})).toBeInTheDocument();
  });

  it('handles empty data', () => {
    const {getByText} = render(
      <AggregateHashrateChart
        serviceUrl="http://localhost:8084"
        data={[]}
        timeRange={{from: 0, to: 2000}}
      />
    );
    expect(getByText(/no data/i)).toBeInTheDocument();
  });
});
```

### Integration Tests

```typescript
describe('Hashrate API', () => {
  it('fetches data from stats-proxy', async () => {
    const data = await fetchHashrateData('http://localhost:8084', 1);
    expect(data.data).toBeDefined();
    expect(Array.isArray(data.data)).toBe(true);
  });

  it('fetches data from stats-pool', async () => {
    const data = await fetchHashrateData('http://localhost:9084', 1);
    expect(data.data).toBeDefined();
    expect(Array.isArray(data.data)).toBe(true);
  });
});
```

---

## Summary

### Two Independent Services

- **web-proxy**: Translator view of miners + stats-proxy
- **web-pool**: Pool view of translators + stats-pool
- Completely separate code bases
- No shared data or interface
- Can be deployed independently

### Same Component Architecture

- Identical component structure in both
- Only API endpoint differs
- Shared utility functions (formatting, calculations)
- Parallel implementation

### Key Implementation Points

1. **Configuration first** - Make serviceUrl and downstreamType configurable
2. **Reusable components** - Generic enough to work with both data types
3. **No coupling** - Don't assume anything about the other service
4. **Independent deployment** - Each service deploys with its own UI
5. **Simple REST API** - No auth, no WebSockets (for now)
6. **10-second polling** - Matches data generation interval
7. **Error resilient** - Graceful degradation on service unavailable

### Suggested Stack

- **Framework**: React or Vue 3
- **Charts**: Chart.js or Recharts
- **State**: Redux/Pinia
- **HTTP**: Fetch API or Axios
- **Testing**: Jest + React Testing Library
- **Styling**: Tailwind CSS or Material-UI
