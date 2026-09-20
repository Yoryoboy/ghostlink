# Destiny AI Companion

## 1. Objetivo del proyecto

Construir una capa inteligente sobre la API oficial de Destiny 2 que permita a una IA comprender el estado real de una cuenta, analizar su inventario, conocer el estado actual del juego y ejecutar acciones sobre los personajes.

El objetivo no es construir simplemente otra interfaz de inventario similar a DIM.

La idea central es invertir el modelo tradicional:

```text
Usuario
   ↓
Lenguaje natural
   ↓
IA
   ↓
Destiny MCP
   ↓
Destiny Core
   ↓
Bungie API
```

En lugar de que el usuario navegue manualmente por Vaults, filtros, perks y actividades, podrá expresar directamente su intención.

Ejemplos:

```text
"Pasame The Last Word al Warlock."

"¿Qué shotguns tengo con One-Two Punch?"

"Quiero hacer una build Stasis melee para Hunter.
Mirá lo que tengo y ayudame a construirla."

"¿Qué me falta para completar esta build?"

"Preparame esta build en el Hunter."

"¿Qué actividades hay esta semana?"

"¿Qué vale la pena farmear esta semana según lo que ya tengo?"

"¿Tengo alguna buena arma Void con Demolitionist?"

"¿Qué duplicados tengo donde una copia sea claramente peor?"
```

El sistema debe convertir estas intenciones en consultas y acciones determinísticas sobre Destiny.

---

# 2. Principios del proyecto

## IA como interfaz

La IA será la principal forma de interacción avanzada.

El usuario no debería necesitar conocer:

* hashes de Bungie;
* IDs de personajes;
* item instance IDs;
* endpoints;
* Manifest;
* Vault buckets;
* sockets;
* estructuras internas de la API.

Puede simplemente pedir:

```text
"Buscame mi mejor escopeta con One-Two Punch."
```

El MCP y el core deben resolver todo lo demás.

---

## Bungie es la fuente de verdad

Nuestra base local será una representación optimizada del estado de Destiny, pero nunca debe considerarse autoritativa.

```text
SQLite
   ↓
cache / índice

Bungie API
   ↓
fuente real
```

Antes de realizar acciones importantes, el sistema debe validar el estado actual contra Bungie.

---

## La sincronización debe ser invisible

Existirá:

```bash
destiny sync
```

pero será principalmente una herramienta de:

* debugging;
* desarrollo;
* mantenimiento;
* refresh manual.

El usuario no debería tener que ejecutar sincronizaciones constantemente.

CLI y MCP decidirán automáticamente cuándo los datos están suficientemente frescos y cuándo necesitan actualizarse.

---

## Separar razonamiento de ejecución

La IA puede decidir qué quiere hacer.

El core será responsable de decidir cómo hacerlo correctamente.

Por ejemplo, la IA puede pedir:

```text
move_item(
  item = "The Last Word",
  destination = "Warlock"
)
```

Pero el core resolverá:

```text
buscar arma
→ localizar instance ID
→ detectar personaje actual
→ personaje → Vault
→ Vault → Warlock
→ verificar resultado
```

La IA no debería implementar lógica de transferencia de Destiny.

---

# 3. Arquitectura general

El proyecto tendrá tres piezas principales:

```text
                    Bungie API
                        │
                        ▼
              ┌──────────────────┐
              │   Destiny Core   │
              │      Rust        │
              └────────┬─────────┘
                       │
             ┌─────────┴─────────┐
             │                   │
             ▼                   ▼
       Destiny CLI          Destiny MCP
          Rust                 Rust
                                   │
                                   ▼
                                  AI
```

Además:

```text
Destiny Core
     │
     ▼
SQLite
```

Una futura UI podría conectarse al mismo core mediante HTTP:

```text
React / TypeScript
        │
        ▼
     HTTP API
        │
        ▼
    Rust Core
```

---

# 4. Lenguajes y tecnologías

## Core

Rust.

Responsabilidades:

```text
Bungie API
OAuth
Manifest
inventario
cache
SQLite
transferencias
equipamiento
world state
build engine
```

Tecnologías probables:

```text
tokio
reqwest
serde
serde_json
sqlx o rusqlite
oauth2
tracing
thiserror
```

---

## CLI

Rust.

Probablemente usando:

```text
clap
```

Debe producir un binario nativo y rápido.

Ejemplos:

```bash
destiny sync

destiny find "last word"

destiny move "last word" warlock

destiny find \
  --weapon shotgun \
  --perk "One-Two Punch"

destiny weekly
```

---

## MCP

Rust utilizando el SDK oficial de MCP.

El MCP será una capa muy delgada sobre `destiny-core`.

No deberá duplicar lógica.

```text
MCP Tool
   ↓
destiny-core
   ↓
resultado
```

---

## Persistencia

SQLite.

Posible estructura:

```text
~/.destiny-ai/

destiny.db
manifest.db
config.toml
logs/
```

---

# 5. Integración con Bungie

La aplicación utilizará la API oficial de Bungie.

Autenticación:

```text
Bungie API Key
+
OAuth 2.0
```

OAuth permitirá acceder al inventario privado y realizar acciones sobre la cuenta.

El sistema deberá manejar automáticamente:

```text
access token
refresh token
token expiration
re-authentication
```

Los secretos nunca deben exponerse al modelo de IA.

---

# 6. Destiny Manifest

Bungie separa la información live de las definiciones estáticas.

Por ejemplo:

```text
Live API

itemHash
itemInstanceId
location
state
sockets
```

mientras que el Manifest permite resolver:

```text
itemHash
   ↓
nombre
tipo
arquetipo
icono
elemento
perks
descripciones
activity definitions
modifier definitions
etc.
```

El Manifest será descargado y almacenado localmente.

El sistema comprobará automáticamente su versión y lo actualizará cuando Bungie publique uno nuevo.

---

# 7. Índice local del inventario

Mantendremos una representación local de:

```text
Vault
Warlock
Hunter
Titan
equipped items
item instances
weapons
armor
perks
sockets
stats
mods
locations
```

Esto permitirá búsquedas extremadamente rápidas.

Ejemplo:

```text
"¿Qué escopetas tengo con One-Two Punch?"
```

podrá resolverse principalmente contra SQLite en lugar de reconstruir toda la cuenta desde Bungie en cada consulta.

---

# 8. Política de sincronización

El sistema utilizará una estrategia de cache con TTL.

Ejemplo conceptual:

```text
cache reciente
→ utilizar SQLite

cache viejo
→ refresh Bungie
→ actualizar SQLite
→ responder
```

La política puede variar según la operación.

| Operación                 | Estrategia                    |
| ------------------------- | ----------------------------- |
| Buscar un arma            | usar cache reciente           |
| Analizar inventario       | refrescar si está viejo       |
| Consultar weekly rotation | utilizar world state reciente |
| Analizar build            | refrescar si es necesario     |
| Transferir objeto         | validar contra Bungie         |
| Equipar objeto            | validar contra Bungie         |
| Prepare build             | refresh obligatorio           |
| `destiny sync`            | refresh forzado               |

Después de operaciones de escritura también se verificará el resultado.

---

# 9. Inventory Engine

El sistema debe entender el inventario como objetos semánticos, no como respuestas crudas de Bungie.

Ejemplo interno:

```json
{
  "name": "Swordbreaker",
  "instanceId": "123456789",
  "type": "Shotgun",
  "element": "Strand",
  "location": "Titan",
  "power": 2000,
  "locked": true,
  "perks": [
    "Pugilist",
    "One-Two Punch"
  ]
}
```

Esto será lo que consume principalmente la IA.

---

# 10. MCP Tools

No queremos exponer la API de Bungie directamente.

En lugar de herramientas como:

```text
bungie_get_profile
bungie_get_item
bungie_transfer_item
```

queremos tools semánticas.

Primera generación:

```text
get_profile

list_characters

search_inventory

inspect_item

move_item
```

Posteriormente, World State:

```text
get_current_week

get_featured_activities

get_activity_details

get_weekly_rewards
```

Después ampliaremos Inventory Actions:

```text
find_weapon

find_armor

equip_item

equip_items
```

Y posteriormente Build Engine:

```text
analyze_build_inventory

prepare_build_plan

execute_build_plan
```

---

# 11. Transferencias

Un comando como:

```text
"Pasame The Last Word al Hunter."
```

debe convertirse en algo parecido a:

```text
buscar The Last Word

↓

encontrar exact itemInstanceId

↓

detectar ubicación

↓

Titan
  ↓
Vault
  ↓
Hunter

↓

verificar
```

El usuario no necesita saber que las transferencias entre personajes pasan internamente por el Vault.

---

# 12. World State

El sistema no solo debe comprender la cuenta del usuario.

También debe comprender qué está ocurriendo actualmente en Destiny.

Este será el **primer gran módulo agregado después del MVP**.

Dominio:

```text
world-state/

milestones
activities
portal
rotations
vendors
```

La API de Bungie no proporciona necesariamente un único endpoint del estilo:

```text
GET /weekly-rotation
```

Por lo tanto reconstruiremos un modelo normalizado usando distintas fuentes oficiales.

Conceptualmente:

```text
Public Milestones
      +
Character Activities
      +
Portal
      +
Activity Graphs
      +
Manifest
      ↓
WeeklyState
```

---

# 13. Weekly State

Nuestro modelo interno podrá representar:

```text
reset actual

inicio del reset

fin del reset

actividades featured

raids

dungeons

Portal

modifiers

challenges

vendors

rewards
```

Esto permitirá consultas como:

```text
"¿Qué salió esta semana?"

"¿Qué raid está featured?"

"¿Qué dungeon está en rotación?"

"¿Qué actividades están disponibles?"

"¿Qué modifiers hay esta semana?"
```

También tendremos acceso desde CLI:

```bash
destiny weekly
```

Ejemplo conceptual:

```text
DESTINY WEEKLY

Reset:
September XX → September XX

Featured Raid:
...

Featured Dungeon:
...

Portal:
...

Modifiers:
...
```

---

# 14. ¿Por qué World State viene inmediatamente después del MVP?

Es una implementación relativamente independiente y de bajo riesgo.

Para ese momento ya tendremos funcionando:

```text
Bungie API
Manifest
HTTP client
serialización
SQLite
MCP
CLI
```

World State reutiliza prácticamente toda esa infraestructura.

Además es principalmente:

```text
lectura
+
resolución de hashes
+
normalización
```

y no necesita inicialmente:

```text
transfer planner complejo
equipamiento
matching de builds
optimización
comparación de stats
acciones multi-item
```

Esto nos permitirá agregar rápidamente mucho valor al MCP sin aumentar demasiado la complejidad.

Además servirá para comprobar que nuestra arquitectura soporta correctamente datos que no pertenecen exclusivamente al inventario.

---

# 15. Inventario + World State

Aunque inicialmente World State podrá funcionar por sí mismo, eventualmente queremos combinarlo con el inventario.

El objetivo será responder:

```text
"¿Qué vale la pena farmear esta semana para mí?"
```

combinando:

```text
Weekly State
     +
Inventory
     +
Build Engine
     +
AI
```

Ejemplo futuro:

```text
Raid X

Te faltan:
- Weapon A
- Weapon B

Ya tienes:
- Weapon C

Tu Weapon C tiene un roll peor que uno de los rolls
recomendados para tu build actual.
```

Esta integración completa llegará después del Build Engine, pero World State se implementará antes.

---

# 16. Equipamiento

La API de Bungie permite equipar objetos.

Esto permitirá comandos como:

```text
"Equipame Conditional Finality."
```

o eventualmente:

```text
"Preparame esta build."
```

El core deberá comprobar las restricciones de Destiny antes de ejecutar la operación.

Esta funcionalidad se implementará después de World State.

---

# 17. Build Engine

Esta será una de las funciones más importantes del proyecto, pero no es necesaria para validar inicialmente la arquitectura.

El sistema podrá comparar una build deseada contra el inventario real.

Ejemplo:

```text
Build requerida

One-Two Punch Shotgun
Grenade Launcher
Twisting Echo Helmet
Twisting Echo Arms
Exotic X
```

contra:

```text
inventario real
```

para producir:

```text
✓ Shotgun One-Two Punch
✓ Grenade Launcher
✓ Twisting Echo Helmet
✗ Twisting Echo Arms
✓ Exotic

Build completion: 80%
```

También podrá identificar sustitutos.

Ejemplo:

```text
No tienes la pieza ideal.

Mejor alternativa disponible:
Arms X
Archetype: Reaver
Stats: ...
```

---

# 18. Prepare Build

Esta será posteriormente una de las herramientas principales.

Entrada:

```text
"Preparame esta build en el Hunter."
```

Primero se genera un plan:

```text
Swordbreaker
Titan → Vault → Hunter

Edge Transit
Vault → Hunter

Helmet
ya está en Hunter

Exotic
Vault → Hunter
```

Después podrá ejecutarse.

Conceptualmente:

```text
prepare_build_plan
        ↓
execute_build_plan
```

Para operaciones grandes preferimos este patrón de planificación + ejecución.

---

# 19. Casos de uso objetivo

Queremos llegar eventualmente a conversaciones como:

```text
"Pasame The Last Word al Warlock."

"Buscame una shotgun con One-Two Punch."

"¿Tengo alguna buena arma Strand con Demolitionist?"

"¿Qué salió esta semana?"

"¿Qué raid está featured esta semana?"

"¿Qué dungeon puedo farmear?"

"Quiero hacer una build Prismatic para mi Warlock.
¿Qué puedo construir con lo que ya tengo?"

"¿Qué me falta para esta build?"

"Preparame la build."

"¿Puedo farmear hoy el arma que necesito?"

"¿Qué actividad me conviene hacer esta semana?"

"¿Qué armas repetidas puedo considerar desmontar?"

"¿Cuál es mi mejor roll de esta arma?"

"Encontrame todas mis armas Void que puedan funcionar con esta build."
```

---

# 20. Fase 1 — MVP

El primer MVP debe demostrar el flujo completo:

```text
Bungie authentication
        ↓
Manifest
        ↓
GetProfile
        ↓
local inventory index
        ↓
CLI
        ↓
MCP
```

Funciones mínimas:

```text
OAuth

Manifest sync

Inventory sync

list_characters

search_inventory

inspect_item

move_item
```

El milestone principal será:

```text
Usuario:
"Pásame The Last Word al Warlock."

AI
 ↓
MCP
 ↓
Destiny Core
 ↓
Bungie
 ↓
The Last Word aparece en Warlock
```

Esto valida:

```text
OAuth
API
Manifest
Inventory
SQLite
CLI
MCP
acciones de escritura
```

---

# 21. Fase 2 — Weekly Rotation / World State

Esta será la implementación inmediatamente posterior al MVP.

Objetivo:

> Permitir que CLI y MCP conozcan qué está ocurriendo actualmente en Destiny.

Implementaremos:

```text
world-state

weekly reset

public milestones

character activities

featured activities

Portal

activity graphs

raid rotation

dungeon rotation

activity modifiers
```

Tools MCP iniciales:

```text
get_current_week

get_featured_activities

get_activity_details

get_weekly_rewards
```

CLI:

```bash
destiny weekly
```

El milestone principal será poder preguntar:

```text
"¿Qué salió esta semana?"
```

y obtener la respuesta a partir de Bungie y nuestro World State, sin necesitar información manual externa.

Esta fase prioriza lectura y normalización antes de continuar aumentando la complejidad de las acciones sobre la cuenta.

---

# 22. Fase 3 — Inventory Actions avanzadas

Después de World State ampliaremos las capacidades de manipulación del inventario.

Implementaremos:

```text
find_weapon

find_armor

equip_item

equip_items

better inventory filters

automatic cache refresh

action verification

lock/unlock

Postmaster
```

Esto permitirá consultas y acciones más potentes como:

```text
"Encontrame todas mis shotguns con One-Two Punch."

"Equipame Conditional Finality."

"Bloqueame este roll."

"Recuperá este objeto del Postmaster."
```

---

# 23. Fase 4 — Build Engine

Una vez estabilizados Inventory Engine, World State y las acciones básicas, construiremos el motor de builds.

Implementaremos:

```text
build requirements

inventory matching

missing items

substitutions

build coverage

roll matching

prepare_build_plan

execute_build_plan
```

El milestone será:

```text
"Quiero hacer esta build."

↓

analizar requisitos

↓

comparar contra inventario

↓

decir qué tengo

↓

decir qué falta

↓

encontrar alternativas

↓

preparar y equipar la build
```

---

# 24. Fase 5 — Inteligencia cruzada

En esta fase combinaremos:

```text
Inventory
+
World State
+
Build Engine
```

Esto habilitará consultas como:

```text
"¿Qué vale la pena farmear esta semana para mí?"

"¿Puedo conseguir esta semana algo que me falte para esta build?"

"¿Qué actividad debería hacer para completar esta build?"

"De las actividades en rotación, ¿qué loot todavía me interesa?"
```

El sistema podrá construir un plan de adquisición personalizado.

Ejemplo:

```text
Quiero Build X

↓

Ya tienes 7/9 piezas.

↓

Una pieza tiene un sustituto adecuado.

↓

La pieza restante se obtiene en Dungeon Y.

↓

Dungeon Y está actualmente en rotación.

↓

Plan de adquisición.
```

---

# 25. Fase 6 — Vault Intelligence

Funciones más avanzadas:

```text
duplicate detection

roll comparison

vault cleanup suggestions

weapon ranking dentro de la propia colección

farm recommendations

gear acquisition planning

collection analysis
```

Ejemplos:

```text
"¿Qué duplicados tengo?"

"De estas cuatro copias, ¿cuál debería conservar?"

"¿Qué armas tengo que cumplen prácticamente la misma función?"

"¿Qué cosas del Vault no utilizo y tienen alternativas claramente mejores?"
```

El sistema nunca desmantelará objetos mediante Bungie.

Podrá analizar y recomendar, pero la decisión final seguirá siendo del usuario.

---

# 26. Orden de desarrollo

La hoja de ruta queda:

```text
FASE 1
MVP
│
├── OAuth
├── Manifest
├── Inventory Sync
├── Search
├── Inspect
├── Move Item
├── CLI
└── MCP
       │
       ▼
FASE 2
WORLD STATE
│
├── Weekly Reset
├── Rotations
├── Featured Activities
├── Portal
└── Modifiers
       │
       ▼
FASE 3
INVENTORY ACTIONS
│
├── Advanced Search
├── Equip
├── Lock
└── Postmaster
       │
       ▼
FASE 4
BUILD ENGINE
│
├── Requirements
├── Matching
├── Missing Items
├── Substitutions
└── Prepare Build
       │
       ▼
FASE 5
CROSS INTELLIGENCE
│
├── Inventory + Weekly State
├── Build + Weekly State
└── Personalized Farming
       │
       ▼
FASE 6
VAULT INTELLIGENCE
```

---

# 27. Lo que NO queremos hacer

El proyecto no debe convertirse en:

```text
otro DIM

un frontend gigante

un proxy tonto de Bungie

un conjunto de endpoints sin semántica

un chatbot que simplemente conozca información general de Destiny
```

El valor está en:

```text
comprender
+
razonar
+
actuar
```

sobre la cuenta real y el estado real del juego.

---

# 28. Visión final

La visión del proyecto puede resumirse así:

```text
              DESTINY
                 │
                 ▼
          Bungie API + Manifest
                 │
                 ▼
            Destiny Core
           ┌─────┴─────┐
           │           │
      Inventory    World State
           │           │
           └─────┬─────┘
                 │
            Build Engine
                 │
           ┌─────┴─────┐
           │           │
          CLI         MCP
                       │
                       ▼
                      AI
                       │
                       ▼
                    Usuario
```

La IA debería poder responder tres grandes preguntas:

```text
¿Qué tengo?

¿Qué está ocurriendo ahora mismo en Destiny?

¿Qué puedo hacer con lo que tengo y con lo que está disponible?
```

Y además debe poder actuar:

```text
buscar

transferir

equipar

preparar builds
```

El objetivo final es crear un verdadero **Destiny AI Companion** conectado a la cuenta real del usuario, capaz de comprender tanto su colección como el estado actual del juego.
