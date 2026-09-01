# Conversation

conversation for stage2 schema, sqlite and relation layer.

> **Numbering.** This conversation was held under the old stage numbering, in which the SQLite
> index was stage 2. The stages were renumbered afterwards — the domain is now stage 2, the CLI
> stage 3, and the index stage 4 — and the schema work discussed here became
> `docs/plans/pre-stage4-refactor.md`. The transcript below is left verbatim: read every "stage 2"
> in it as the index, i.e. stage 4, and every "stage 3"/"stage 4" as stages 2 and 3.

-- User

Then, let me actually think about the scenario and reconsider root_id and reply_to.
  The reason why I wanted to defer stage 2 was I wasn't convinced the extra layer that sqlite file provides, and need to decide what it actually have to do.

  Basically this project is to view markdown file differently. The original idea came from markdown linter reporting error having title frontmatter and H1 heading in body as an error. This is understandable, but then, I noticed that filename and title or h1 are sementically the same but located in different places.

  As folder structure with markdown files for knowledge base doesn't function well, I would rather use UUID in the filename so that a file itself. Since I'm a very obsessive person, I thought it would be good idea for frontmatters to have a specific key-value pair for one workspace.

  Then, the cost of reading file is expensive, I thought using sqlite as a cache to enhance it would be a good idea.

  What is implemented so far is a working prototype, but not exactly satisfactory as we move forward, and I would like to update design changes before we start working on stage 2.

  First, here are decision I made and I regret is about workspace.toml.
  Here is how it looks and what I think this should be:

  // current
  schema_version = 1

  [workspace]
  id = <UUIDv4>
  kind = "jot"
  name = "workspace"

  [schema]
  frontmatter = [
      "title",
      "relation:root",
      "relation:reply_to",
      "relation:quote",
  ]

  // adjustment
  schema_version = 1

  [workspace]
  id = <UUIDv4>
  name = "workspace"
  type = "<jot|plain>"                        # it makes more sense to be name `type` rather then kind

  [[schema.frontmatter]]
  key = "title"           # key for frontmatter key
  type = "document:title" # type for value of key

  [[schema.frontmatter]]
  key = "relation:root"
  type = "relation:root" # assumes it to be valid UUID for a note in workspace

  [[schema.frontmatter]]
  key = "relation:reply_to"
  type = "relation:reply_to"

  [[schema.frontmatter]]
  key = "relation:quote"
  type = "relation:quote"

  To explain,

- we can rename workspace.kind to workspace.type
- we can have schema.frontmatter to be more explicit.

  I don't want you to implement it, but have a look and comment on it, as I have additional extension based on this.

  Searched for 2 patterns, ran 1 shell command

-- Claude

디자인만 검토하고 구현은 하지 않겠습니다.

1. kind → type 이름 변경

- 순수하게 이름만 보면 타당합니다. 다만 실제로 그 필드가 무엇을 결정하는지 먼저 확인할 가치가 있습니다.
- 현재 WorkspaceKind(workspace.rs:117)가 지배하는 것은 두 가지입니다: 파일명 정책(UUID 이름 vs 자유 이름)과 스레드 유무.
- 그런데 아래 type 시스템이 들어오면 "스레드가 있는가"는 스키마가 relation:* 역할을 선언하는가로 답할 수 있게 됩니다. 그러면 남는 건 파일명 정책뿐입니다.
- 즉 이 필드는 점점 type(무엇인가)보다 naming 또는 layout(파일을 어떻게 배치하는가)에 가까워집니다. 지금 type으로 옮겼다가 나중에 또 바꾸는 것보다, 이 필드가 최종적으로 무엇을 결정하는지 정한 뒤에 이름을 붙이는 편이 낫습니다.
- 실무적 사족: Rust에서 type은 예약어라 r#type 또는 #[serde(rename = "type")]가 필요합니다. 막는 이유는 아니지만 kind가 관용적으로 쓰이는 이유이기도 합니다.

1. 타입 붙은 frontmatter — 진짜 이점은 "명시성"이 아닙니다

말씀하신 근거("more explicit")보다 훨씬 강한 근거가 있고, 그게 이 프로젝트의 출발점과 정확히 맞물립니다.

- 지금 코드에서 의미는 키 이름에 하드코딩돼 있습니다. frontmatter.rs:121의 pub const TITLE: &str = "title", 그리고 INTERPRETED_KEYS(:137)가 전부입니다. FrontmatterSchema는 keys: Vec<String>, 즉 허용 키 목록일 뿐입니다(:159).
- 그래서 "제목이 어디 있는가"라는 질문의 답이 "title"이라는 리터럴 문자열입니다.
- 그런데 이 프로젝트의 출발점은 파일명과 title과 H1이 의미상 같은 것인데 위치만 다르다는 관찰이었습니다. 그 관찰의 논리적 귀결은 의미(역할)를 위치(키 이름)에서 떼어내는 것입니다.
- type = "document:title"이 하는 일이 정확히 그겁니다. 키를 heading이든 name이든 제목이든 쓸 수 있고, core는 여전히 "이 키가 제목 역할을 한다"를 압니다.
- 이걸 근거로 내세우세요. "더 명시적"은 취향 논쟁이 되지만, "역할과 키 이름의 분리"는 프로젝트의 전제에서 곧바로 따라나오는 결론입니다.

1. 초안의 문제 — key와 type이 3/4에서 동일합니다

key = "relation:reply_to"
type = "relation:reply_to"

- 값이 같다는 건 그 자리에서 타입이 정보를 전혀 나르지 않는다는 뜻입니다. title만 다르고 나머지 셋은 자기 자신을 반복합니다.
- 두 가지 중 하나로 읽힙니다:
  - (a) type을 생략하면 key가 기본값이다 — 그러면 관례를 따르는 vault의 manifest가 다시 짧아집니다.
  - (b) relation 타입이 아직 덜 설계됐다 — 제 생각엔 이쪽입니다. 아래를 보세요.
- 어느 쪽이든, 네 항목 중 세 개에서 타입이 무의미해 보이는 초안은 설득력이 약합니다. 타입이 키 이름과 다를 수 있다는 걸 예시가 스스로 보여줘야 합니다.

1. relation:root를 relation:reply_to와 같은 종류로 타이핑하면 안 됩니다

앞서 논의한 tree(root_id) vs thread() 문제가 여기서 정확히 재등장합니다.

- reply_to는 실제 간선(edge)입니다. 사용자가 만든 사실이고, 트리 모양의 유일한 근거입니다.
- root는 비정규화된 캐시(denormalized cache)입니다. create 시점에 계산돼 저장되고 재계산되지 않습니다(workspace.rs:894). recompute_root(:736)가 언제든 reply_to로부터 다시 구할 수 있습니다.
- 둘을 relation:*라는 같은 네임스페이스에 나란히 두면, 스키마가 "이 둘은 같은 종류의 사실"이라고 주장하는 것이 됩니다. 아닙니다. 하나는 원본, 하나는 파생입니다.
- 타입 시스템을 도입하는 김에 이걸 표현할 수 있습니다. 예를 들어:

[[schema.frontmatter]]
key = "relation:reply_to"
type = "relation:parent"      # 진짜 간선

[[schema.frontmatter]]
key = "relation:root"
type = "derived:thread_root"  # reply_to로부터 계산됨

- 그러면 derived:* 타입은 rebuild가 다시 채워도 되는 값이라는 규칙이 생깁니다. 그게 stage 2에서 필요했던 바로 그 결정입니다 — "reply_to가 권위이고 root_id는 최적화다"를 코드 주석이 아니라 스키마가 선언하게 됩니다.
- 부가 효과: edited_at이 rebuild 불변식에서 면제되는 이유도 같은 어휘로 설명됩니다.

1. 항목에 빠진 것들

배열-of-tables([[schema.frontmatter]])를 고른 건 옳습니다 — 순서가 보존되고(stage 4의 $EDITOR 템플릿이 순서에 의존합니다), 나중에 키당 필드를 추가할 자리가 생깁니다. 지금은 비어 있지만 곧 필요할 것들:

- 개수(cardinality) — relation:quote는 하나인가 여럿인가? 현재 스키마의 quoted_id는 단수입니다. 여러 노트를 인용하고 싶어지는 순간 파일 형식이 바뀝니다. 타입이 이걸 말해줘야 합니다.
- 필수 여부 — title은 없어도 되고 relation:root는 있어야 합니다(없으면 open_note가 복구합니다). 지금은 missing_relation_keys(:207)라는 별도 함수가 하드코딩으로 답하고 있습니다. 타입 항목이 말할 수 있으면 그 함수가 사라집니다.
- 역할 중복 — 두 키가 모두 document:title을 주장하면? 배열 순서가 우선순위를 줄 수 있습니다(첫 번째 승리). 정해두지 않으면 나중에 임의로 결정됩니다.

1. 타입 집합은 반드시 열려 있어야 합니다

- 이 프로젝트의 핵심 규칙은 모르는 frontmatter 키는 보존한다입니다. 타입을 도입하면 "모르는 타입은 거부한다"로 미끄러지기 쉽습니다.
- 규칙을 지금 못박아 두세요: core가 이해하지 못하는 type은 경고이고, 그 키는 알 수 없는 키와 똑같이 그대로 보존된다. 거부가 아닙니다.
- 이유는 같습니다. 새 버전이 붙인 타입을 옛 바이너리가 만나는 일이 반드시 생기고, 그때 파일이 손상되면 안 됩니다.

1. schema_version은 2가 돼야 합니다

- [schema] frontmatter = [...](문자열 배열)에서 [[schema.frontmatter]](테이블 배열)로 가는 건 호환되지 않는 형식 변경입니다. 같은 버전 번호로는 안 됩니다.
- 기존 machinery는 이미 절반을 갖고 있습니다: Error::UnsupportedSchemaVersion이 "더 새 버전이 쓴 파일"을 거부합니다. 없는 건 반대 방향, 즉 1을 읽어서 2로 올리는 경로입니다.
- 프로토타입 단계이니 선택지는 둘입니다: (a) 읽을 때 v1을 v2로 승격(문자열 키 → type이 키와 같은 항목), (b) v1 지원 중단. (a)가 몇 줄이고, 스스로가 첫 사용자이니 해볼 가치가 있습니다.

1. 이게 stage 2 질문에 답합니다

원래 미루신 이유가 *"sqlite라는 층이 실제로 무엇을 해야 하는지 확신이 없다"*였는데, 이 변경이 그 답의 형태를 바꿉니다.

- 현재 stage 2 스키마는 역할을 컬럼으로 하드코딩합니다 — notes 테이블에 root_id, reply_to_id, quoted_id가 각각 컬럼입니다. 새 관계를 하나 추가하려면 마이그레이션이 필요합니다.
- 타입이 있으면 간선을 일반화할 수 있습니다 — edges(src_id, role, dst_id) 한 테이블. role은 스키마가 선언한 관계 타입. 새 관계는 manifest 한 줄이고 DB 변경이 없습니다.
- 그리고 이건 인덱스의 정당성 자체를 선명하게 합니다: 파일이 진실이고, DB는 스키마가 선언한 역할들을 파일에서 뽑아 캐시한 것입니다. derived:* 타입은 DB가 다시 계산해도 되는 것, 나머지는 파일만이 답할 수 있는 것.
- 즉 "인덱스가 무엇을 담아도 되는가"가 취향이 아니라 스키마에서 따라나오게 됩니다. 그게 지금 없는 규칙이고, 미루신 불편함의 정체라고 봅니다.

정리

- 방향은 옳습니다. 근거를 "명시성"에서 **"역할과 키 이름의 분리"**로 바꾸면 훨씬 강해집니다.
- 초안에서 고칠 곳 하나: relation:root를 relation:reply_to와 같은 종류로 두지 마세요. 파생값이라는 사실이 타입에 드러나야 하고, 그게 stage 2의 미결정 사항을 스키마 차원에서 해소합니다.
- workspace.kind 이름 변경은 그 필드가 최종적으로 무엇을 결정하는지 정한 다음에 하시는 게 좋겠습니다.

확장안이 있다고 하셨는데, 위 4번(파생 vs 원본)과 5번(개수·필수)이 그 확장과 충돌하는지 궁금합니다. 들어보고 다시 검토하겠습니다.

-- User

I think what you said are reasonable.

## 1

I though `workspace.type` would explain this better, but if the reserved name is the issue I can either come up with a new keyword or we can even consider dropping `workspace.kind` as plain is virtually not meaningful to have when there are better options for managing markdown files.

## 2

I got the ideas for this project from obsidian's newer feature called base that provides notion's strong database feature using yaml frontmatter.

## 3 & 4

For `key` and `type` with "relation:reply_to", it is actually both. We can only leave type and make key for frontmatter to follow that of type's value. Let's think of those value strings in type I provided as an example as reserved keyword specifically for type. Also, I need to reconsider the design of relation type. I will go over that it more detailed manner - if we unify jot not to handle plain, we can assume all notes to possibly have comment and quote relation I originally intended to implement.

## 5

```toml
[[file.frontmatter]]
type = "relation:quote_to"
optional = true

[[file.frontmatter]]
key = "created_at"
type = "document:created_at" # type can be string

[[file.frontmatter]]
key = "visited"
type = "string:date" # type can be string

[[file.frontmatter]]
key = "summary"
type = "string" # type can be string

[[file.frontmatter]]
key = "source"
type = "string:url" # or url

[[file.frontmatter]]
key = "tags"
type = "array"
```

I also thought about those and here are my proposed ideas

- for cadinality, we can have an array type that corresponds to yaml array for keys like `tags`, for example. Those `document:<...>` or `relation:<...>` are reserved keywords, and I thought `relation:quote` would have a single value (maybe `relation:quote_to` make is more consistent with `relation:reply_to`)
- we can introduce `optional` key that has false by default (when omitted)
- for duplicated role issue, I think validation logic to toml file at interface level would be enough - we can restrict reserved keyword fields not to have a duplicated value.

## 6

As mentioned, field (=key-value pair) not found in workspace.toml should be kept and error should be handled within the application level (interface layers like cli, tui, or gui)

I remember mentioning the frontmatter order, but not sure if that is implemented or not, so let me clarify it here.

Let's say we have `title: document:title` and `source: string:url?` declared in the schema, and we have `---\ntitle: blahblah\nsummary: foobarbaz\nsource:<url>` Then, jot should reorder this to title-summary-source (so schema order first, unspecified field at the end) Then, when the file is opened from interface layer, the error saying unspecified value should be displayed

## 7

Since we are working on prototype before making this into version 2 is pointless as it is not public yet.

## 8

As mentioned in `## 1` if we drop kind and assume jot will support the relation, it might be better for sqlite layer to have a new table that shows the `quote_to` and `reply_to` relation.

Then, we can think an sqlite file as a cache, having tables for `notes` and `relation`. Then, for this relation table, we can store

- (`note_a_id`, `reply_to`, `note_b_id`) for reply_to relation
- (`note_a_id`, `quote_to`, `note_b_id`) for quote_to relation
- (`note_a_id`, `point_to`, `note_b_id`) even for wiki link style
  , having columns like `note_from_id`, `note_to_id`, `note_to_id` I think this will help the sqlite file truly work as a cache.

Then, I think we can now drop `relation:root` and make it only available within sqlite layer (`note_a_id`, `root_for`, `note_b_id`) for quote_to relation.

-- Claude

문서 읽었습니다. 먼저 코드에서 몇 가지 확인했고, 그 결과 §6은 이미 구현돼 있습니다.

## §6 — 이미 구현돼 있습니다

말씀하신 재정렬은 설계할 것이 없습니다. `Frontmatter::try_render_with`(`frontmatter.rs:478-503`)가
이미 정확히 그 순서로 씁니다.

- 스키마 선언 순서 → 스키마에 없는 `INTERPRETED_KEYS` → 알 수 없는 키는 원문 그대로 끝에.
- `FrontmatterSchema` 주석이 `"Order is the whole content of this type"`라고 못박고 있습니다.

새로운 건 뒤쪽 절반뿐입니다 — **선언되지 않은 키를 인터페이스에서 알린다.** 이건
`SyncReport.problems`가 이미 나르는 것과 같은 종류라 `Problem` variant 하나면 됩니다. 다만 이름은
`error`가 아니라 problem이어야 합니다. 선언 안 된 키는 보존 규칙상 정상 상태이고, 파일을 거부할
근거가 아닙니다.

## §8 — 핵심이고, 열려 있던 질문에 답합니다

`relation:root`를 파일에서 빼고 DB에서만 갖는 안은 지난번에 제기한 `tree(root_id)` vs `thread()`
문제를 **해소합니다**. reply_to 하나만 권위가 되고 root는 파생이 되니 두 답이 갈라질 여지가
없어집니다. 부수 효과도 큽니다:

- `open_note`의 `relation:root` 복구 write가 사라집니다 → **sync가 진짜로 read-only**가 됩니다.
  stage2.md가 요구하던 성질이 설계에서 따라나옵니다.
- stage3-4 log 소견 #2(부모에 `relation:root`가 없을 때의 fallback)가 통째로 없어집니다.
- `recompute_root`(`workspace.rs:736`)는 이미 존재하고, **이미 cycle을 감지해서 `ReplyCycle`을
  냅니다.** 필요한 로직은 이미 쓰여 있고, 지금은 파일당 조상들을 다시 load하는 방식일 뿐입니다.
  snapshot/DB에서는 이미 읽은 record 위의 memoized walk라 비용이 사실상 없습니다.

그런데 네 가지를 정하셔야 합니다.

### (a) `root_for`를 같은 relation 테이블에 넣으면 안 됩니다

이게 위 4번에서 지적된 것과 **정확히 같은 실수가 한 층 아래에서 재발하는** 자리입니다.

- `reply_to`, `quote_to`, `point_to`는 어떤 파일이 주장하는 **사실**입니다. 행 하나에 그것을 쓴
  파일이 대응됩니다.
- `root_for`는 reply_to의 **transitive closure**입니다. 주장한 파일이 없습니다.
- 같은 테이블에 같은 모양으로 두면 원본과 파생이 다시 구별 불가능해집니다. `notes.root_id`를 파생
  컬럼으로 두거나, 이름이 파생임을 말하는 별도 테이블로 두세요.

### (b) provenance가 비대칭입니다

- `reply_to` / `quote_to` — frontmatter에서 옵니다. 스키마가 선언하고, 키를 다시 써서 바꿉니다.
- `point_to` — **본문**에서 옵니다. 스키마가 선언하지 않고, 끌 수도 없고, 산문을 고쳐야 바뀝니다.

한 테이블에 담는 것 자체는 옳습니다. 다만 "이 간선을 바꾸려면 무엇을 쓰는가"와 "스키마가 이걸 끌 수
있는가"의 답이 다르므로, `source` 컬럼이든 문서화된 규칙이든 하나는 필요합니다.

### (c) purge 시 subtree가 쪼개집니다 — 설계 결정입니다

현재 `root_id`가 "한 번 배정되고 재계산되지 않는" 것은 **의도된 것**이고, 중간 노트를 purge해도
subtree가 묶여 있게 하려는 목적입니다. root를 파생으로 만들면 그 성질이 사라집니다: 자식들은 고아가
되고 각자가 자기 root가 됩니다.

- 제 판단으로는 **쪼개지는 쪽이 정직합니다.** 묶어주던 근거(파일)가 실제로 없어졌는데 묶여 있는 건
  DB만 아는 사실이고, 그게 이 프로젝트가 금지하는 상태입니다. timeline의 orphan clause가 이미 이
  경우를 처리합니다.
- 다만 이건 stage 3이 명시적으로 내린 결정을 뒤집는 것이므로, 부작용이 아니라 결정으로 기록돼야
  합니다.

### (d) cycle이 이제 더 급해집니다

파생 root를 SQL로 구하면 recursive CTE인데, cycle에서 돕니다. 지금 in-memory walk는 반복 지점에서
멈추기만 하고 아무 말도 안 합니다(post-stage4 "Still open"). 정하세요: **scan 시점에 감지 →
`Problem`으로 보고 → root 계산에서 제외.** `recompute_root`에 이미 있는 감지를 scan으로 옮기는
일입니다.

**stage2.md에 대한 귀결:** `notes` 테이블의 `root_id`/`reply_to_id`/`quoted_id` 세 컬럼과 별도
`links` 테이블이 `notes` + `relations` 하나로 바뀝니다. 새 관계 추가가 migration이 아니라 manifest
한 줄이 됩니다. 스키마 절은 다시 써야 합니다.

## §7 — 여기는 동의하지 않습니다

"공개 전이라 버전 2는 의미 없다"는 **릴리스 버전과 파일 포맷 버전을 섞고 있습니다.** 다른 것입니다.

- `[schema] frontmatter = [...]` → `[[schema.frontmatter]]`는 호환되지 않는 변경이고, 지금
  `jot-workspaces/`에 그 형식의 manifest가 실제로 있습니다. 보호 대상은 "공개된 사용자"가 아니라
  **본인의 현재 vault**입니다.
- 버전을 1로 두면 옛 manifest가 "더 옛 버전이 쓴 파일"이 아니라 그냥 이해하기 어려운 parse error를
  냅니다. 올리면 그 메시지가 저절로 맞아집니다.
- 마이그레이션할 사람이 본인 하나뿐이라 비용이 가장 쌉니다. 지금 안 올리면 나중엔 못 올립니다.

반대로 **노트 파일 쪽은 걱정할 것이 없습니다.** 기존 노트의 `relation:root`는 스키마에서 빠지는
순간 그냥 unknown key가 되어 보존되고 무시됩니다. 보존 규칙이 그대로 마이그레이션이 됩니다 —
여기서 그 규칙이 값을 합니다.

## §5 — 타입 항목

- **`optional`의 기본값이 false라면, "필수"가 무엇을 *하는지*를 정해야 합니다.** 지금은
  `missing_relation_keys`가 하드코딩으로 답하고, `open_note`가 복구합니다. 필수의 의미는 **"open 시
  복구한다"**여야 하고, **절대 "파일을 거부한다"가 되면 안 됩니다.** 사용자가 쓴 파일을 안 읽는
  것은 이 프로젝트가 유일하게 하지 않기로 한 일입니다.
- 부수적으로, 기본이 필수면 대부분의 항목이 `optional = true`를 달게 됩니다. `required = true`
  opt-in이 읽기에 낫습니다.
- **`type = "array"`는 본인 규칙을 깹니다.** `string:url`은 원소를 말하는데 `array`는 컨테이너만
  말합니다. `array:string`이나 `multiple = true` 중 하나로 맞추세요. `tags`가 무엇의 배열인지
  타입이 답해야 합니다.
- **중복 역할 검증은 동의합니다.** 그리고 여기서 좋은 규칙 하나가 나옵니다: **manifest는 설정이므로
  엄격하게 검증하고, 노트 파일은 사용자 데이터이므로 절대 거부하지 않는다.** 이걸 명시해두면 타입
  시스템의 엄격함이 노트 파일로 새는 것을 구조적으로 막습니다.

## §3 & 4 — type이 정체성, key는 기본값

이 형태가 맞고, 원래 지적("3/4에서 key와 type이 같다")도 이걸로 해소됩니다. 한 가지만: **각 예약
타입이 자신의 canonical default key를 선언해야 합니다.** `document:title` → `title`인데
`relation:reply_to` → `relation:reply_to`라면 기본값 규칙이 "타입 문자열 그대로"가 아닙니다.
타입별 표로 정해두지 않으면 나중에 임의로 결정됩니다.

## §1 — kind 제거

동의합니다. 그리고 이제 **일관됩니다**: 관계를 하나도 선언하지 않은 workspace가 곧 "plain"이었던
것입니다. 구분이 필드에서 스키마로 옮겨갑니다 — 처음에 드린 논거("이 필드가 결국 무엇을
결정하는가")의 결론이 그것입니다. `type` 예약어 문제도 필드가 없어지면 같이 없어집니다.

## §2 — Bases

한 가지 차이는 의식하시는 게 좋겠습니다. Bases는 **임의의** frontmatter 위에 나중에 얹는 query
view입니다. 여기서 하려는 건 역할을 **미리 선언하는 contract**입니다. 후자가 강한 대신, 선언되지
않은 것에 대해 관대해야 한다는 부담을 집니다 — §6의 problem과 §5의 "노트는 거부하지 않는다"가 그
부담을 지는 자리입니다.

## 규모에 대해

이건 stage 2의 준비가 아니라 **stage 1의 스키마 결정을 다시 쓰는 작업**입니다. `frontmatter.rs`의
`TITLE`/`INTERPRETED_KEYS`/`RELATION_KEYS` 상수(`:121-143`)가 전부 타입 표로 흡수되고, manifest
직렬화(`workspace.rs:202-219`)와 `FrontmatterSchema` 전체가 바뀝니다. stage 1b acceptance suite도
여기 걸립니다.

**stage 1c로 이름 붙여서 stage 2 앞에 두시길 권합니다.** SQLite를 얹은 뒤에 하면 스키마
마이그레이션까지 같이 끌고 가야 합니다.

---

정리하면, 반대하는 건 §7 하나이고, §8에서 결정이 필요한 건 (a) `root_for`를 파생으로 분리,
(c) purge 시 subtree 분할을 받아들일지, (d) cycle 정책 — 셋입니다.

### 열린 질문

1. §8(a) — `root_id`를 `notes`의 파생 컬럼으로 둘지, 별도 테이블로 둘지.
2. §8(c) — purge가 subtree를 쪼개는 것을 받아들이는지.
3. §8(d) — cycle을 scan 시점 `Problem`으로 보고하고 root 계산에서 제외하는 것이 맞는지.
4. §7 — `schema_version`을 2로 올릴지.
5. §5 — `optional` 기본값을 뒤집을지(`required` opt-in), `array`의 원소 타입 표기.

-- User

(a & b)
Ok, then let's just keep `reply_to`. the current API ensures a note to be its children anyway, let me revisit later if that really matters. same for `root_for`, let's discard that as well as `point_to` if I consider to use wiki-link style.

(c)
Can you explain me the explicit decision made in stage 3 - commit and/or specific part of documentation. Now, as we decide to remove `root_id` or `root_for`, I understand purge splits the subtree being `쪼개지는 쪽이 정직합니다.` I'm not against this idea and this makes sense, but the original idea of thread from those micro blogs, which provides indication that there was a chain an one post is gone. Although those services have soft-delete, I think for this project I need to handle it in the interface level (plus, I don't think I will delete notes that often anyway)

(d)
Can you give me an exact scenary that recursive CTE take places? Because `quote_to` is basically many-to-one toward the note being targeted, and `reply_to` is basically uni-directional in the interface level. Unless a user manually modifies it, it won't take places. If that's what you are concerning, suggest me how cycle can be detected.

Also, we definitely need to update stage2.md for plan

(7)
for 7, then let's make it version 2 right now as this is still a prototype. However, we can lower this number to 1 for public version as its default.

(5)

- I like `required = true`, let's use that rather than `optional = true`
- What required is for is just have the key always there regardless of value provided or not. e.g., right now `title` is optional, but I want to see `---\ntitle:\n---` with no value found in the file frontmatter.
- I think `array:string` looks good for now, but I concern as there might be a scenario `array:string:date` is what I might want later. The way obsidian use is `text` and `multitext` for frontmatter so let's have type named `text` and `multitext` rather than string so that we could have `multitext:url` or `text:date` later.

(§3 & 4 — type이 정체성, key는 기본값)
What I thought is we can omit key when type is provided and the value of type will be used for frontmatter. `schema.frontmatter.key` is the name of key for field in frontmatter and `schema.frontmatter.type` is what the value of the field should be verified as.

```yaml
title: hello world # type is `document:title`
# ^ this is the key
```

So, 'key: title' & 'type: document:title' example is like the above

(§1 — kind 제거)
This one is good and we decide to delete `type` or `kind` issue.

(§2 — Bases)
I just realized that the base is more like the query view rather than the strict one - so just think of it as where I got the idea.

(Other)

For the `notes` table, I want to give more flexibility to the system using json column. So those types that are `document` or `relation` in `schema.frontmatter` will live in the table, but not additional fields from (`## 5`) like `visited`, or `source` will live in a json column. I understand the draw back of searching json, but it will give me more flexibility.

-- Claude

## (c) stage 3의 결정 — 어디에 있고, 얼마나 단단한가

찾아보니 **생각보다 약한 결정입니다.**

문서:

- `docs/plans/stage3.md:44-45` — *"`relation:root` is assigned once, at creation… It is never
  recomputed, so purging a middle note leaves the subtree grouped."*
- `docs/plans/stage3.md:161` — acceptance: *"Purge that note instead: children stay live, the
  reference reports `Deleted`, and the subtree is still grouped under the original `root_id`."*

코드:

- `workspace.rs:870` — `create`의 doc comment, "which is what keeps a subtree grouped when a note in
  the middle of it is purged."
- `workspace.rs:1076` — `purge`의 doc comment, 같은 주장.
- `workspace.rs:3112` — `purging_removes_one_file_and_leaves_the_children_live_and_grouped`, 이걸
  고정하는 유일한 테스트.

**그런데 커밋을 보면 `bfb2352 docs: add staged implementation plan`입니다.** 즉 stage 1 이전, 계획을
처음 쓸 때 함께 쓰인 문장이고, 구현 중에 발견된 것도 dogfooding으로 검증된 것도 아닙니다. 지금까지
이 프로젝트에서 뒤집힌 결정들(short id, workspace id 버전, workspace selector)이 전부 그 부류였습니다
— 계획 시점의 추측이고, 실제로 써보기 전까지는 근거가 없던 것들.

### 그리고 말씀하신 micro blog의 성질은 `root_id`가 제공한 적이 없습니다

이게 핵심입니다. "체인이 있었고 하나가 사라졌다"는 표시는 `root_id`에서 나오지 않습니다.
**살아남은 자식의 dangling `reply_to`에서 나옵니다.**

- 자식은 `relation:reply_to: <purged-id>`를 그대로 들고 있습니다. 그 id는 vault에 없으므로
  `Ref::Deleted`로 해석됩니다. 이건 `root_id`를 지워도 **파일에 그대로 남습니다.**
- 인터페이스는 그걸 읽어서 "삭제된 노트에 대한 답글"이라고 그리면 됩니다. 말씀하신 "interface
  level에서 처리하겠다"가 맞고, **추가 비용이 0입니다.**

형제 관계까지도 살아남습니다:

- R에 자식 A, B가 있고 R을 purge하면, A와 B는 각각 root가 됩니다. 하지만 **둘 다 `reply_to: R`을
  들고 있습니다** — 같은 없는 id를 가리킵니다.
- 그래서 "사라진 같은 부모를 가리키는 것들"로 여전히 묶을 수 있습니다. 묶는 정보가 `root`가 아니라
  `reply_to`에 있었던 것뿐입니다.

진짜로 잃는 것은 **2단계 이상**입니다. R을 purge하고 A도 purge하면, A의 자식들은 R을 모릅니다.
그런데 그건 체인이 실제로 두 번 끊어진 경우이고, 저는 그때 모르는 게 정직하다고 봅니다.

**결론: 이 결정은 뒤집어도 됩니다.** 지키려던 성질이 실은 `reply_to`가 지키고 있었습니다.
stage3.md:44-45와 :161, `workspace.rs:3112` 테스트는 stage 1c에서 고쳐 쓰면 됩니다.

## (d) recursive CTE — 지적이 맞고, 제가 필요 없는 걸 끌어들였습니다

먼저 정정합니다. **recursive CTE는 root를 SQL로 계산하기로 할 때만 등장합니다.** 그건 제가 제안한
선택지였지 주어진 조건이 아니었습니다. 그렇게 하지 않으면 됩니다.

### 대신 이렇게

**root는 scan 시점에 Rust에서 계산해서 컬럼에 넣습니다. SQLite는 결과를 읽기만 합니다.**

- scan은 어차피 모든 노트를 이미 메모리에 올립니다. 거기서 `reply_to`를 위로 걸으면 파일 재로드가
  없습니다.
- 알고리즘은 이미 있습니다 — `recompute_root`(`workspace.rs:736`)의 `seen` 벡터 방식 그대로,
  대상만 파일에서 record로 바꾸면 됩니다.
- memoize하면 전체 O(n)입니다. 걷다가 이미 root가 정해진 조상을 만나면 거기서 멈춥니다.
- **cycle 감지가 공짜입니다.** 걷는 데 `seen`이 필요하고, `seen`에 다시 들어오는 게 곧 cycle입니다.
  따로 만드는 게 아니라 걷기의 부산물입니다.

즉 CTE 질문 자체가 사라집니다. DB는 계속 멍청한 캐시로 남습니다.

### cycle이 실제로 생기는 경로

"사용자가 손으로 고치지 않는 한 안 생긴다"는 맞습니다. 다만 **이 프로젝트에서 그건 예외가 아니라
전제입니다** — 파일이 진실이고 사람이 직접 편집하는 것이 이 도구의 존재 이유입니다. 구체적으로:

1. **자기 자신 참조.** UUID를 복사해 붙이다 `relation:reply_to`에 자기 id를 넣는 것. 1-cycle이고
   제일 흔합니다.
2. **손으로 만든 2-cycle.** A가 B의 답글인데 B를 열어 `reply_to: A`를 넣는 경우.
3. **파일 복사.** 노트를 복사해 파일명 UUID만 바꾸고 frontmatter는 그대로 두는 경우. duplicate id
   문제와 같은 출처(sync client, copy-paste)입니다.
4. **git / 동기화 병합.** vault가 git이나 동기화 폴더에 있고(README가 상정합니다) 두 기기가 서로
   다르게 `reply_to`를 고친 뒤 병합될 때.
5. **미래의 `reparent`.** stage3.md:46-48이 명시적으로 남겨둔 명령입니다. subtree를 다시 쓰는
   중간에 중단되면 그 상태가 cyclic일 수 있습니다.

빈도가 낮은 건 맞습니다. 다만 비용이 비대칭입니다 — 감지가 걷기의 부산물이라 **거의 공짜**인데,
놓치면 무한 루프이거나 조용히 사라진 노트입니다.

### 그래서 정책

- scan 중 cycle을 만나면 `Problem::ReplyCycle { path, id }`로 **보고**합니다. error가 아닙니다 —
  파일 하나가 나머지를 못 읽게 만들면 안 된다는 규칙 그대로입니다.
- 해당 노트는 **자기 자신을 root로** 삼습니다. 즉 timeline에 최상위로 **보입니다.** 사라지지
  않습니다. 고치려면 보여야 합니다.
- 이건 post-stage4 "Still open"에 남아 있던 항목을 닫습니다: 지금 `Error::ReplyCycle`은
  `open_note`에서만 발생하고 읽기 경로는 잘린 트리를 조용히 그립니다.

## (a & b) 동의

`root_for` 폐기, `point_to`는 wiki-link을 실제로 쓰기로 할 때 다시. relation 테이블은 당분간
`reply_to`와 `quote_to` 둘입니다. provenance 비대칭 문제(b)도 `point_to`가 빠지면서 같이 사라집니다
— 남는 둘은 모두 frontmatter 출처입니다. wiki-link을 넣을 때 다시 꺼내면 됩니다.

## (7) 버전 2 — 다만 나중에 1로 되돌리는 건 권하지 않습니다

지금 2로 올리는 건 좋습니다. 공개 시점에 1로 되돌리는 것에는 반대합니다.

- 그러면 **`schema_version = 2`가 두 가지를 뜻하게 됩니다** — 지금의 형식과, 공개 후의 어떤 형식.
  파일 하나가 빠져나가는 순간(백업, 블로그, 공유 repo, 몇 달 뒤 복원한 디스크) 영구히 모호합니다.
- 얻는 건 "공개 v1"이라는 **표시상의 깔끔함뿐**이고, 그건 릴리스 버전이 이미 담당합니다. manifest의
  `schema_version`은 릴리스 버전과 무관한 별개의 축입니다.
- 숫자는 단조 증가하게 두세요. 비용이 0이고, 모호함은 비쌉니다.

## (5) 타입 항목

**`required = true`** — 좋습니다. 그리고 말씀하신 의미("값이 있든 없든 키는 항상 있다")는 **이미
있는 메커니즘입니다.**

- `frontmatter.rs:569-576`의 `Absent { Skip, Placeholder }`가 정확히 그것이고, 지금은 `$EDITOR`
  버퍼에만 `Placeholder`가 쓰입니다(`:450`).
- `required`는 이걸 **렌더 전체의 모드에서 키별 속성으로 옮기는 것**입니다. 새 개념이 아니라 이미
  있는 스위치를 스키마가 키마다 켜는 것입니다.
- 그래서 `required`는 **검증 규칙이 아니라 렌더 규칙입니다.** 파일을 거부하지 않습니다. 제가 앞서
  걱정한 것이 여기서 자동으로 해소됩니다.

한 가지 충돌: `frontmatter.rs:126-127`이 *"an **empty** `relation:reply_to:` is not [a real state],
and is never written"*이라고 못박고 있습니다.

- `title:`이 비는 건 무해합니다 — 없는 제목과 빈 제목은 같은 뜻입니다.
- `relation:reply_to:`가 비는 것은 "최상위"와 구별돼야 합니다. 규칙 하나로 해결됩니다: **빈 값은
  언제나 부재로 파싱된다.** 그러면 `required`는 순수하게 표시상의 것이 되고 의미를 바꾸지 않습니다.
  `relation:*`에 `required`를 붙이는 것도 안전해집니다.

**`text` / `multitext`** — 동의합니다. Obsidian과 맞고, `array:string:date` 같은 중첩을
`multitext:date`로 평평하게 만듭니다. 경계 하나만: 이건 **cardinality를 타입 이름에 넣는** 방식이라
`relation:*`이 나중에 다중값이 되면(부모 여럿) `multirelation:*`이 필요해집니다. 관계는 단일값이라고
정하셨으니 지금은 문제없고, 그 가정이 깨지는 날 바뀌는 자리라는 것만 적어둡니다.

## (§3 & 4) 제가 과하게 설계했습니다

`key`를 생략하면 키가 `type` 문자열 **그대로**라는 규칙이면 충분합니다. 타입별 default key 표는
필요 없습니다 — `document:title`은 어차피 `key = "title"`을 항상 쓸 것이고, `relation:*`은 key와
type이 같아서 생략하는 것이니, 규칙 하나로 두 경우가 다 설명됩니다. 제 제안은 철회합니다.

## (Other) JSON 컬럼

방향은 좋고, 우려하신 검색 문제는 **나중에 해결 가능합니다.** SQLite는 `json_extract`에 대한
generated column을 만들고 거기에 인덱스를 걸 수 있습니다. 즉 어떤 필드가 뜨거워지면 그때
**스키마 변경 없이** 인덱스를 붙일 수 있습니다. 지금 결정을 미뤄도 되는 종류의 것입니다.

다만 선을 다르게 긋는 걸 제안합니다. `document:*` vs 나머지가 아니라:

> **컬럼은 인덱스 자신의 쿼리가 필요로 해서 존재한다. 타입이 특별해서가 아니다.**

- `document:*` 타입이 늘어날 때마다 컬럼이 늘면, 새 타입 추가가 다시 migration이 됩니다 — 이 설계로
  탈출하려던 바로 그것입니다.
- timeline / search / 정렬이 실제로 필요로 하는 것만 컬럼입니다: `id`, `title`, `state`,
  `created_at`, `edited_at`.
- `relation:*`은 애초에 컬럼이 아닙니다 — relations 테이블로 갑니다.
- 나머지 선언된 필드는 전부 JSON. `document:created_at`도 마찬가지입니다(id에서 디코드하므로 사실
  중복입니다).

그리고 정해야 할 것 하나: **JSON 컬럼에 선언된 키만 넣을지, 선언 안 된 키까지 넣을지.**

- **선언된 것만** 넣기를 권합니다. JSON 컬럼의 목적은 질의이고, 무엇을 질의할 수 있는지가 스키마
  선언의 존재 이유입니다.
- 그러면 §6의 problem이 행동 가능해집니다: *"이 키는 선언되지 않았습니다 — 검색하려면 선언하세요."*
  선언에 보상이 생깁니다.
- 선언 안 된 키는 여전히 **파일에 보존**됩니다. 인덱스에 없을 뿐입니다. 파일이 진실이라는 규칙과
  정확히 맞습니다.

---

## 남은 것

거의 다 닫혔습니다. 남은 것은 둘입니다.

1. **JSON 컬럼에 선언된 키만 넣는가** — 위의 권고에 동의하시는지.
2. **컬럼 선정 기준** — "쿼리가 필요로 하는 것만"으로 가는지, `document:*`은 전부 컬럼인지.

이 둘이 정해지면 `stage1c.md`(스키마·타입·manifest)와 `stage2.md`(notes + relations + JSON,
scan 시점 root 계산, cycle 정책)를 다시 쓰겠습니다. (c)와 (d)의 결과로 stage2.md의 스키마 절과
stage3.md:44-45 / :161, `workspace.rs:3112`도 함께 고쳐야 합니다.

-- User


