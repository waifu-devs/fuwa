# The economy: currencies, earning and the shop

A server can have its own **currencies** (coins, gems, points...), each
with its own name and icon. Members earn them by what they do in the server,
can send them to each other where a currency allows it, and spend them in
the server's **shop** on roles, profile items and anything else the server
names. People with the right permissions give and take currency, audited,
and watch the server's finances on one page. Agents do all of it through
the same permissions.

`EconomyService` is the protocol (`proto/fuwa/v1/economy.proto`); the
shared types (`Currency`, `EarnRule`, `ShopListing`, `Balance`,
`LedgerEntry`) and events are in `types.proto`. Apps check the `economy`
feature (docs/compatibility.md).

## Permissions

| Permission | Lets you |
| --- | --- |
| Manage Economy (`MANAGE_ECONOMY`) | Make, change and delete currencies and their earn rules, run the shop, see the finance page and everyone's balances and history. Listing a role also takes Manage Roles and a rank above the role, as handing it out would. |
| Grant Currency (`GRANT_CURRENCY`) | Give currency to a member or to everyone with a role, or take it away, with a reason. Read anyone's wallet. Every grant is in the audit log. |

Both are server-wide only. A new server's Admin role has Manage Economy;
Grant Currency makes currency out of nothing, so an owner hands it out. Everyone else may hold currency,
claim the daily reward, send transferable currencies and buy from the shop,
unless they're timed out or haven't agreed to the rules yet.

## Currencies

A `Currency` has a name (1 to 32 characters), a description, an icon (a
standard emoji's characters, or one of the server's custom emoji, which
wins while it's there; apps fall back to 🪙), a position, whether members
may send it to each other (`transferable`, off by default), and its earn
rules. A server may have as many as it likes. Every change sends the whole
list as `CurrenciesUpdated`, so members see how to earn as soon as it
changes. Deleting a currency takes every balance, ledger entry and listing
in it (roles and items already bought stay with their buyers).

Amounts are whole numbers; balances never go below zero.

## Earning

Each currency has at most one `EarnRule` per action, with an amount, and:

| Action | When | Extra |
| --- | --- | --- |
| `MESSAGE` | Sending a message in the server's own text, announcement or thread channels (not secure ones, not shared from another server, not webhooks). | `cooldown_seconds` between two earnings, so spam doesn't pay. |
| `REACTION_RECEIVED` | Someone else reacts to your message. | `cooldown_seconds`, like messages. Taking a reaction off doesn't take it back. |
| `VOICE_MINUTE` | Each minute in one of the server's voice channels with at least one other person, while you're not deafened. | |
| `DAILY` | Pressing Claim (`ClaimDaily`), once per UTC day. | `streak_bonus` more for each day in a row after the first, up to `streak_max_days`. Missing a day starts the streak again. |
| `JOIN` | Joining the server. | Once per person ever: leaving and joining again doesn't pay again. |
| `ONBOARDING` | Finishing the server's onboarding. | Once per person ever. |

Every rule may have a `daily_cap`: the most it gives one person in a UTC
day. Agents never earn by actions (they're given or sent currency).

Earning happens inside the write that did the thing (the message, the
reaction, the join), so a message that AutoMod blocks earns nothing. Voice
is in memory, so the shard holding a server credits the people in its
voice channels once a minute in one write (`economy::spawn_voice_earnings`).
The state that throttles earning (when someone last earned by a rule, and
how much today) is the server file's `currency_earnings`.

## The shop

A `ShopListing` sells one thing for one currency:

- **A role** (`ROLE`): bought once, kept for good. Like the roles
  onboarding hands out, it may carry nothing that moderates or manages the
  server (a role that gains such a permission can't be bought until it
  loses it again). Someone with Manage
  Roles can still take it away like any role; buying it again pays again.
  Nobody may buy a role they already have.
- **A profile item** (`PROFILE_ITEM`): one of the server's effects or
  decorations (docs/profile-items.md), bought once. While an item has a
  listing (available or not), only those who bought it may put it on in
  the server; taking the listing down makes it free to wear again, and
  those who bought it keep it (`profile_item_owners`).
- **Anything else** (`CUSTOM`): by name and description, bought as often as
  there's stock. The server hands it out by hand or through an agent, told
  by `PurchaseMade`.

A listing has a price, an optional stock (how many are left), how many were
sold, a position and whether it's available: members buy, and apps show
them, only available listings. Every member gets every listing (`ListShop`,
`ShopUpdated`), so apps know which profile items stay locked. Buying checks the price the app
showed (`expected_price`), so a price that just changed is never paid
unseen. Buying, the stock and the balance are one write.

Manager changes send the whole shop as `ShopUpdated`; a purchase sends the
listing (`ShopListingUpdated`) to everyone, `BalanceUpdated` to the buyer,
and `PurchaseMade` to the buyer and members with Manage Economy.

## Moving currency

- **Transfers** (`Transfer`): a member sends some of a transferable
  currency to another member, with an optional note. Both sides get a
  ledger entry and `BalanceUpdated`.
- **Grants** (`Grant`, Grant Currency): to one member or to everyone with a
  role (the server's id for @everyone). An amount below zero takes it away,
  stopping at zero. The reason goes to the people it reached (in their
  ledger) and to the audit log (`CURRENCY_GRANT`: the member or role, the
  currency and the amount). A grant to one member sends them
  `BalanceUpdated`; a grant to a role sends one `WalletsChanged`, and apps
  holding a wallet read it again.

## The ledger and the finance page

Every change to a balance is a `LedgerEntry` in the server file's
`currency_ledger`: who, how much, the balance after, and why (earned by an
action, granted by someone, sent to or from someone, spent on a listing).
Members read their own (`ListLedger`); Manage Economy reads anyone's or
everyone's.

`GetEconomyOverview` is the finance page, for one currency over the last
1 to 365 days: what's held now and by how many people, what was earned
(and by which action), granted, taken, spent and sent, the top holders,
spenders and earners among current members, the listings bought most, and
a total per day. Apps draw it as tiles, a chart of the days and ranked
lists, beside the whole ledger.

## Privacy

A balance is its owner's: other members never see it, nor what someone
bought. Managers (Manage Economy) and granters (Grant Currency) do, as part
of running the server. Nothing about the economy leaves the server: it isn't
in the usage signal or the health report, and other instances never see it.
Deleting an account deletes its balances, earnings, ledger and what it
bought.

## In the apps

Both apps show, where the server has currencies:

- **The wallet**: the person's balances in the channel sidebar's foot (the
  currency's icon and the count, which rolls when it changes), opening the
  **Shop**: balances with Claim buttons for daily rewards, the listings to
  buy (owned ones marked), the person's history, and Send for transferable
  currencies.
- **Server settings > Economy** (Manage Economy): currencies with their
  icon, description, transfer switch and earn rules; the shop's listings.
- **Server settings > Finances** (Manage Economy): the finance page, with
  the whole ledger.
- **Give currency** (Grant Currency): on members' profile cards and
  right-click menus, and to a role from the finance page.
- Profile items for sale show their price and a padlock in the server
  profile picker until bought.
