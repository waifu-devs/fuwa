//! The tools for running a server: its settings, channels, roles, invites,
//! emoji, webhooks, AutoMod, the way in, shared channels, profile items, Mod
//! View, the economy and boards.
//! Each wraps one gRPC call whose arguments are the request itself, read
//! from proto3 JSON (field names as in the .proto), and answers with the
//! response the same way. The input schema is built from the request's
//! descriptor, comments included, so a tool always takes what its call
//! takes. Enums are written as their short names ("text", "send_messages"),
//! as the other tools write them; the full names work too.
//!
//! Nothing here decides what an agent may do: the call checks its roles as
//! it would anyone's, and rank still limits what it can touch.

use std::sync::LazyLock;

use prost_reflect::{
    DescriptorPool, DeserializeOptions, DynamicMessage, EnumDescriptor, FieldDescriptor, Kind as FieldKind,
    MessageDescriptor, SerializeOptions,
};
use serde_json::{Map, Value, json};
use tonic::Status;

use super::{Cx, RpcError, call, tools};
use crate::pb;

static POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(crate::proto::FILE_DESCRIPTOR_SET).expect("the API's descriptor set reads")
});

fn descriptor(name: &str) -> MessageDescriptor {
    POOL.get_message_by_name(name).unwrap_or_else(|| panic!("{name} is a fuwa.v1 message"))
}

/// What a tool does, for the annotations apps read before running it.
#[derive(Clone, Copy, PartialEq)]
enum Effect {
    Reads,
    Writes,
    /// Can't be undone: apps should ask first.
    Destroys,
}

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// The request message's full name.
    request: &'static str,
    effect: Effect,
}

/// The tools and how each one calls the API: `name => client::Type.method(Request)`.
macro_rules! tools {
    ($($name:literal => $client:ident :: $type:ident . $method:ident ( $req:ident ), $effect:ident, $title:literal, $description:literal;)*) => {
        const TOOLS: &[Tool] = &[$(
            Tool {
                name: $name,
                title: $title,
                description: $description,
                request: concat!("fuwa.v1.", stringify!($req)),
                effect: Effect::$effect,
            },
        )*];

        async fn run(cx: &Cx, tool: &Tool, args: Value) -> Result<Result<Value, Status>, RpcError> {
            Ok(match tool.name {
                $($name => {
                    let req: pb::$req = request(tool.request, args)?;
                    call!(cx, $client::$type.$method(req)).map(|r| response(tool.request, &r))
                })*
                name => return Err(RpcError::invalid(format!("there's no tool {name:?}"))),
            })
        }
    };
}

tools! {
    // The server.
    "update_server" => server_service_client::ServerServiceClient.update_server(UpdateServerRequest), Writes,
        "Change server settings",
        "Changes the server's settings; fields left out stay as they are. Pictures (icon_url, banner_url) are \
         links upload_picture returned. Needs Manage Server.";
    "get_server_usage" => server_service_client::ServerServiceClient.get_server_usage(GetServerUsageRequest), Reads,
        "Server usage",
        "How much the server holds (members, channels, messages, files) against its limits. Needs Manage Server.";
    "update_member" => server_service_client::ServerServiceClient.update_member(UpdateMemberRequest), Writes,
        "Change a member",
        "Sets a member's nickname in the server (empty clears it): this agent's own with Change Nickname, \
         someone else's with Manage Nicknames, only below this agent's highest role.";
    "list_bans" => server_service_client::ServerServiceClient.list_bans(ListBansRequest), Reads,
        "List bans",
        "Who's banned from the server, newest first, with why. Needs Ban Members.";
    "list_audit_log" => server_service_client::ServerServiceClient.list_audit_log(ListAuditLogRequest), Reads,
        "Read the audit log",
        "What managers and moderators did in the server, newest first, a page at a time. Needs View Audit Log.";
    "get_mcp_access" => agent_service_client::AgentServiceClient.get_mcp_access(GetMcpAccessRequest), Reads,
        "Which agents use MCP here",
        "Which of the server's agents its managers let use it through MCP: all, chosen ones or none. Only \
         people change it, never an agent.";

    // Channels.
    "get_channel" => channel_service_client::ChannelServiceClient.get_channel(GetChannelRequest), Reads,
        "Get a channel",
        "One channel with everything about it, its permission overwrites included.";
    "create_channel" => channel_service_client::ChannelServiceClient.create_channel(CreateChannelRequest), Writes,
        "Create a channel",
        "Makes a channel (text, voice, announcement or secure) or a category. parent_id puts it in a category. \
         Needs Manage Channels; permission_overwrites also need Manage Roles.";
    "update_channel" => channel_service_client::ChannelServiceClient.update_channel(UpdateChannelRequest), Writes,
        "Change a channel",
        "Renames a channel, sets its topic or slow mode, or moves it in or out of a category; fields left out \
         stay as they are. Needs Manage Channels.";
    "reorder_channels" => channel_service_client::ChannelServiceClient.reorder_channels(ReorderChannelsRequest), Writes,
        "Reorder channels",
        "Puts every channel in a new order, and in or out of categories, at once: list every channel of the \
         server (list_channels), once each, in display order. Needs Manage Channels.";
    "set_channel_permissions" => channel_service_client::ChannelServiceClient.set_channel_permissions(SetChannelPermissionsRequest), Writes,
        "Set channel permissions",
        "Replaces a channel's (or category's) permission overwrites: per role (@everyone's id is the server's) \
         or member, what's allowed and denied there. Needs Manage Roles, and only permissions this agent has.";
    "delete_channel" => channel_service_client::ChannelServiceClient.delete_channel(DeleteChannelRequest), Destroys,
        "Delete a channel",
        "Deletes a channel and every message in it, for good. Needs Manage Channels.";

    // Roles.
    "create_role" => role_service_client::RoleServiceClient.create_role(CreateRoleRequest), Writes,
        "Create a role",
        "Makes a role, ranked just above @everyone. color is 0xRRGGBB as a number. It can hold only \
         permissions this agent has. Needs Manage Roles.";
    "update_role" => role_service_client::RoleServiceClient.update_role(UpdateRoleRequest), Writes,
        "Change a role",
        "Renames a role, sets its color, hoist and mentionable, and turns permissions on (grant) or off \
         (revoke); fields left out stay as they are. Only roles ranked below this agent's highest, and only \
         permissions it has. Needs Manage Roles.";
    "reorder_roles" => role_service_client::RoleServiceClient.reorder_roles(ReorderRolesRequest), Writes,
        "Reorder roles",
        "Ranks the roles: every role but @everyone, highest first. Roles at or above this agent's highest \
         must stay where they are. Needs Manage Roles.";
    "delete_role" => role_service_client::RoleServiceClient.delete_role(DeleteRoleRequest), Destroys,
        "Delete a role",
        "Deletes a role ranked below this agent's highest and takes it off everyone who has it. Needs Manage Roles.";

    // Invites.
    "list_invites" => invite_service_client::InviteServiceClient.list_invites(ListInvitesRequest), Reads,
        "List invites",
        "The server's invite links and who made them. Needs Manage Server.";
    "create_invite" => invite_service_client::InviteServiceClient.create_invite(CreateInviteRequest), Writes,
        "Create an invite",
        "Makes an invite: people join at https://<instance>/invite/<code>. Optionally into a channel, for a \
         number of uses or seconds. Needs Create Invite.";
    "delete_invite" => invite_service_client::InviteServiceClient.delete_invite(DeleteInviteRequest), Destroys,
        "Delete an invite",
        "Stops an invite working. Needs Manage Server, unless this agent made it.";

    // Emoji (list_emojis and create_emoji are with the other tools).
    "rename_emoji" => emoji_service_client::EmojiServiceClient.update_emoji(UpdateEmojiRequest), Writes,
        "Rename an emoji",
        "Gives one of the server's custom emoji a new name. Needs Manage Emoji.";
    "delete_emoji" => emoji_service_client::EmojiServiceClient.delete_emoji(DeleteEmojiRequest), Destroys,
        "Delete an emoji",
        "Deletes one of the server's custom emoji and its picture. Needs Manage Emoji.";

    // Webhooks.
    "list_webhooks" => webhook_service_client::WebhookServiceClient.list_webhooks(ListWebhooksRequest), Reads,
        "List webhooks",
        "The server's webhooks with their tokens (anyone holding one can post). Needs Manage Webhooks.";
    "create_webhook" => webhook_service_client::WebhookServiceClient.create_webhook(CreateWebhookRequest), Writes,
        "Create a webhook",
        "Makes a webhook that posts in a channel; programs post at /webhooks/<server id>/<webhook id>/<token>. \
         avatar_url is a link upload_picture returned for webhook_avatar. Needs Manage Webhooks.";
    "update_webhook" => webhook_service_client::WebhookServiceClient.update_webhook(UpdateWebhookRequest), Writes,
        "Change a webhook",
        "Sets a webhook's name, picture and channel (send all three). Needs Manage Webhooks.";
    "reset_webhook_token" => webhook_service_client::WebhookServiceClient.reset_webhook_token(ResetWebhookTokenRequest), Destroys,
        "Reset a webhook's token",
        "Gives a webhook a new token; its old address stops working at once. Needs Manage Webhooks.";
    "delete_webhook" => webhook_service_client::WebhookServiceClient.delete_webhook(DeleteWebhookRequest), Destroys,
        "Delete a webhook",
        "Deletes a webhook; the messages it posted stay. Needs Manage Webhooks.";

    // AutoMod.
    "list_automod_rules" => auto_mod_service_client::AutoModServiceClient.list_auto_mod_rules(ListAutoModRulesRequest), Reads,
        "List AutoMod rules",
        "The server's AutoMod rules, and the moderation providers a provider rule can use. Needs Manage Server.";
    "save_automod_rule" => auto_mod_service_client::AutoModServiceClient.save_auto_mod_rule(SaveAutoModRuleRequest), Writes,
        "Save an AutoMod rule",
        "Adds a rule (without an id) or replaces one (with its id). Up to 6 keyword rules, one mention spam, \
         one link and one provider rule. Needs Manage Server.";
    "test_automod_rule" => auto_mod_service_client::AutoModServiceClient.test_auto_mod_rule(TestAutoModRuleRequest), Reads,
        "Try an AutoMod rule",
        "What a rule, saved or not, would make of some text, sending nothing. A provider rule asks its \
         provider, so the text goes to the provider's host. Needs Manage Server.";
    "delete_automod_rule" => auto_mod_service_client::AutoModServiceClient.delete_auto_mod_rule(DeleteAutoModRuleRequest), Destroys,
        "Delete an AutoMod rule",
        "Deletes one of the server's AutoMod rules. Needs Manage Server.";

    // The way in.
    "get_join_form" => join_service_client::JoinServiceClient.get_join_form(GetJoinFormRequest), Reads,
        "Read the rules and questions",
        "The server's rules (members agree to them before talking) and the questions applicants answer.";
    "set_join_form" => join_service_client::JoinServiceClient.set_join_form(SetJoinFormRequest), Writes,
        "Set the rules and questions",
        "Replaces the server's rules and application questions. Turn applications on with update_server. \
         Needs Manage Server.";
    "list_applications" => join_service_client::JoinServiceClient.list_applications(ListApplicationsRequest), Reads,
        "List applications",
        "Applications to join waiting for an answer, with what people wrote. Needs Kick Members.";
    "review_application" => join_service_client::JoinServiceClient.review_application(ReviewApplicationRequest), Writes,
        "Answer an application",
        "Lets an applicant in, or turns them down with a reason they see. Needs Kick Members.";
    "get_welcome_screen" => join_service_client::JoinServiceClient.get_welcome_screen(GetWelcomeScreenRequest), Reads,
        "Read the welcome screen",
        "What new members see first: a description and channels to start in.";
    "set_welcome_screen" => join_service_client::JoinServiceClient.set_welcome_screen(SetWelcomeScreenRequest), Writes,
        "Set the welcome screen",
        "Replaces what new members see first. Needs Manage Server.";
    "get_onboarding" => join_service_client::JoinServiceClient.get_onboarding(GetOnboardingRequest), Reads,
        "Read onboarding",
        "The steps new members go through: picks that give roles and channels, the rules, a hello.";
    "set_onboarding" => join_service_client::JoinServiceClient.set_onboarding(SetOnboardingRequest), Writes,
        "Set onboarding",
        "Replaces the server's onboarding steps. Needs Manage Server.";

    // Shared channels (docs/shared-channels.md).
    "list_shared_channels" => shared_channel_service_client::SharedChannelServiceClient.list_connections(ListConnectionsRequest), Reads,
        "List shared channels",
        "The server's channels shared with other servers, both ways, with requests waiting, share codes and \
         people kept out. Needs Manage Server.";
    "create_share_code" => shared_channel_service_client::SharedChannelServiceClient.create_share_code(CreateShareCodeRequest), Writes,
        "Share a channel",
        "Makes a code another server's managers use to show one of this server's channels there; this server \
         approves them before it shows. Needs Manage Server, and Manage Channels in the channel.";
    "delete_share_code" => shared_channel_service_client::SharedChannelServiceClient.delete_share_code(DeleteShareCodeRequest), Destroys,
        "Delete a share code",
        "Stops a share code working. Needs Manage Server.";
    "preview_share" => shared_channel_service_client::SharedChannelServiceClient.preview_share(PreviewShareRequest), Reads,
        "Look at a share code",
        "Whose channel a share code leads to, and what showing it here means. Needs Manage Server.";
    "accept_share" => shared_channel_service_client::SharedChannelServiceClient.accept_share(AcceptShareRequest), Writes,
        "Ask to show a shared channel",
        "Asks to show another server's channel here; it appears once that server approves. Needs Manage Server.";
    "review_share" => shared_channel_service_client::SharedChannelServiceClient.review_share(ReviewShareRequest), Writes,
        "Answer a share request",
        "Approves or turns down another server's request to show one of this server's channels. Needs Manage \
         Server, and Manage Channels in the channel.";
    "update_shared_channel" => shared_channel_service_client::SharedChannelServiceClient.update_connection(UpdateConnectionRequest), Writes,
        "Change what a guest server may do",
        "Sets what another server's people may do in one of this server's shared channels: some of \
         send_messages, embed_links and attach_files. Needs Manage Server, and Manage Channels in the channel.";
    "block_from_shared_channel" => shared_channel_service_client::SharedChannelServiceClient.block_from_channel(BlockFromChannelRequest), Writes,
        "Keep someone out of a shared channel",
        "Keeps one person from another server out of one of this server's shared channels, or lets them \
         back with blocked false. Needs Kick Members.";
    "disconnect_shared_channel" => shared_channel_service_client::SharedChannelServiceClient.disconnect(DisconnectRequest), Destroys,
        "Disconnect a shared channel",
        "Ends a shared channel's connection, or withdraws or turns down a request. The guest's channel goes \
         away; the messages stay with the home server. Needs Manage Server.";

    // Profile items the server offers (docs/profile-items.md).
    "list_profile_items" => profile_item_service_client::ProfileItemServiceClient.list_server_profile_items(ListServerProfileItemsRequest), Reads,
        "List profile items",
        "The profile effects and avatar decorations the server offers its members.";
    "create_profile_item" => profile_item_service_client::ProfileItemServiceClient.create_server_profile_item(CreateServerProfileItemRequest), Writes,
        "Create a profile item",
        "Offers members a profile effect or an avatar decoration (a picture upload_picture returned for \
         decoration). Needs Manage Server.";
    "update_profile_item" => profile_item_service_client::ProfileItemServiceClient.update_server_profile_item(UpdateServerProfileItemRequest), Writes,
        "Change a profile item",
        "Changes one of the server's profile effects or decorations. Needs Manage Server.";
    "delete_profile_item" => profile_item_service_client::ProfileItemServiceClient.delete_server_profile_item(DeleteServerProfileItemRequest), Destroys,
        "Delete a profile item",
        "Deletes one of the server's profile effects or decorations and takes it off everyone wearing it. \
         Needs Manage Server.";

    // Mod View (docs/mod-view.md).
    "report_message" => moderation_service_client::ModerationServiceClient.report_message(ReportMessageRequest), Writes,
        "Report a message",
        "Reports a message to the server's moderators, with a reason and an optional note. Reporting it again \
         changes the reason and note.";
    "report_member" => moderation_service_client::ModerationServiceClient.report_member(ReportMemberRequest), Writes,
        "Report a member",
        "Reports someone who is or was in the server to its moderators, with a reason and an optional note.";
    "list_mod_cases" => moderation_service_client::ModerationServiceClient.list_mod_cases(ListModCasesRequest), Reads,
        "List Mod View cases",
        "Messages AutoMod caught and messages or members people reported, newest first, filtered by status, \
         kind or who they're about. Needs Use Mod View.";
    "get_mod_case" => moderation_service_client::ModerationServiceClient.get_mod_case(GetModCaseRequest), Reads,
        "Get a Mod View case",
        "One case with every report in it (who reported and why) and the message as it is now. Needs Use Mod View.";
    "update_mod_case" => moderation_service_client::ModerationServiceClient.update_mod_case(UpdateModCaseRequest), Writes,
        "Close or reopen a case",
        "Marks a case resolved or dismissed, with an optional reason, or opens it again. Doesn't act on anyone. \
         Needs Use Mod View.";
    "get_member_overview" => moderation_service_client::ModerationServiceClient.get_member_overview(GetMemberOverviewRequest), Reads,
        "Look into a member",
        "Someone's membership, ban, cases, what moderators did to them and the notes about them. Needs Use Mod \
         View.";
    "list_member_messages" => moderation_service_client::ModerationServiceClient.list_member_messages(ListMemberMessagesRequest), Reads,
        "A member's messages",
        "What someone wrote, newest first, in the channels this agent can see. Needs Use Mod View.";
    "find_people" => moderation_service_client::ModerationServiceClient.find_people(FindPeopleRequest), Reads,
        "Find someone to look into",
        "People who are or were in the server (members first, then who left, was banned or applied) by part of \
         their name, username or nickname, or their id. Needs Use Mod View.";
    "add_mod_note" => moderation_service_client::ModerationServiceClient.add_mod_note(AddModNoteRequest), Writes,
        "Write a moderator note",
        "Adds a note about someone that only members with Use Mod View see.";
    "delete_mod_note" => moderation_service_client::ModerationServiceClient.delete_mod_note(DeleteModNoteRequest), Destroys,
        "Delete a moderator note",
        "Deletes one of this agent's notes, or anyone's with Manage Server. Needs Use Mod View.";

    // The economy (docs/economy.md).
    "list_currencies" => economy_service_client::EconomyServiceClient.list_currencies(ListCurrenciesRequest), Reads,
        "List the server's currencies",
        "The server's currencies in order, with their icons and how members earn them.";
    "list_shop" => economy_service_client::EconomyServiceClient.list_shop(ListShopRequest), Reads,
        "List the shop",
        "Everything the server's shop lists, in order: roles, profile items and custom things, with prices, \
         stock and how many were bought. Members can buy only the available ones.";
    "get_wallet" => economy_service_client::EconomyServiceClient.get_wallet(GetWalletRequest), Reads,
        "Get a wallet",
        "Balances in every currency and the profile items bought: this agent's, or a member's with Manage \
         Economy or Grant Currency.";
    "list_ledger" => economy_service_client::EconomyServiceClient.list_ledger(ListLedgerRequest), Reads,
        "List balance changes",
        "Every change to balances, newest first: this agent's, a member's, or everyone's (user_id \"*\") with \
         Manage Economy. Filter by currency and kind; page with before_id.";
    "transfer_currency" => economy_service_client::EconomyServiceClient.transfer(TransferRequest), Writes,
        "Send currency to a member",
        "Sends some of this agent's own balance of a currency members may send to each other, with a note.";
    "buy_listing" => economy_service_client::EconomyServiceClient.buy(BuyRequest), Writes,
        "Buy from the shop",
        "Buys an available listing with this agent's own balance.";
    "grant_currency" => economy_service_client::EconomyServiceClient.grant(GrantRequest), Writes,
        "Give or take currency",
        "Gives an amount of a currency to a member, or to everyone with a role (the server's id for \
         @everyone); an amount below zero takes it away, never below zero. The reason goes to them and the \
         audit log. Needs Grant Currency.";
    "get_economy_overview" => economy_service_client::EconomyServiceClient.get_economy_overview(GetEconomyOverviewRequest), Reads,
        "The server's finances",
        "One currency over the last days: what's held and by how many, earned (by action), granted, taken, \
         spent and sent, the top holders, spenders and earners, the listings bought most and each day's \
         totals. Needs Manage Economy.";
    "create_currency" => economy_service_client::EconomyServiceClient.create_currency(CreateCurrencyRequest), Writes,
        "Create a currency",
        "Adds a currency with its name, icon (an emoji, or one of the server's emoji by id), description, \
         whether members may send it, and its earn rules (set replace_earn). Needs Manage Economy.";
    "update_currency" => economy_service_client::EconomyServiceClient.update_currency(UpdateCurrencyRequest), Writes,
        "Change a currency",
        "Changes what's set in the change; earn rules are replaced only with replace_earn. Needs Manage Economy.";
    "delete_currency" => economy_service_client::EconomyServiceClient.delete_currency(DeleteCurrencyRequest), Destroys,
        "Delete a currency",
        "Deletes a currency with every balance, ledger entry and listing in it. Needs Manage Economy.";
    "create_shop_listing" => economy_service_client::EconomyServiceClient.create_shop_listing(CreateShopListingRequest), Writes,
        "List something in the shop",
        "Sells a role (one that moderates or manages nothing, below this agent's own; also needs Manage Roles), \
         one of the server's profile items, or anything else by name, for a price in one currency. Needs \
         Manage Economy.";
    "update_shop_listing" => economy_service_client::EconomyServiceClient.update_shop_listing(UpdateShopListingRequest), Writes,
        "Change a listing",
        "Changes a listing's name, description, price, stock, position or whether it's available. Needs \
         Manage Economy.";
    "delete_shop_listing" => economy_service_client::EconomyServiceClient.delete_shop_listing(DeleteShopListingRequest), Destroys,
        "Take a listing down",
        "Deletes a listing; what was bought stays with its buyers. Needs Manage Economy.";
    "list_gacha_banners" => economy_service_client::EconomyServiceClient.list_gacha_banners(ListGachaBannersRequest), Reads,
        "List gacha banners",
        "The server's gacha banners in order: price, multi-pull, rarity tiers with their rates (in millionths), \
         prizes with their weights and each one's chance, pity, what duplicates do, and when each is open; \
         and this agent's pulls and pity count on each.";
    "pull_gacha" => economy_service_client::EconomyServiceClient.pull(PullRequest), Writes,
        "Pull on a gacha banner",
        "Pulls once (count 1) or the banner's multi-pull with this agent's own balance, and says what dropped.";
    "list_gacha_pulls" => economy_service_client::EconomyServiceClient.list_gacha_pulls(ListGachaPullsRequest), Reads,
        "List gacha pulls",
        "Pulls with every draw and its rolls, newest first: this agent's, a member's, or everyone's (user_id \
         \"*\") with Manage Economy. Filter by banner; page with before_id.";
    "create_gacha_banner" => economy_service_client::EconomyServiceClient.create_gacha_banner(CreateGachaBannerRequest), Writes,
        "Create a gacha banner",
        "Adds a banner: name, currency and pull price, an optional multi-pull (size and price), rarity tiers \
         rarest first with ids you choose and rates in millionths adding up to 1000000, prizes (a shop \
         listing_id, or a currency_id and amount) each in a tier with a weight, optional pity (pity_pulls and \
         pity_tier_id), duplicates (KEEP, REFUND with refund_currency_id and refund_amount, or SKIP_OWNED), \
         max_pulls per member, starts_at and ends_at, and available. Mark listings gacha_only with \
         update_shop_listing so they can't be bought. Needs Manage Economy (and Manage Roles for role prizes).";
    "update_gacha_banner" => economy_service_client::EconomyServiceClient.update_gacha_banner(UpdateGachaBannerRequest), Writes,
        "Change a gacha banner",
        "Replaces a banner with the one given, as list_gacha_banners has it with your changes (keep prize ids \
         to keep them). Needs Manage Economy.";
    "delete_gacha_banner" => economy_service_client::EconomyServiceClient.delete_gacha_banner(DeleteGachaBannerRequest), Destroys,
        "Delete a gacha banner",
        "Deletes a banner; its pulls stay in the log and what was won stays with its winners. Needs Manage \
         Economy.";

    // Boards (docs/boards.md). Comments are send_message with the card's id as thread_id.
    "get_board" => board_service_client::BoardServiceClient.get_board(GetBoardRequest), Reads,
        "Read a board",
        "A board channel's columns, labels and cards (not archived), in order, with each card's body message \
         and the people named.";
    "list_archived_cards" => board_service_client::BoardServiceClient.list_archived_cards(ListArchivedCardsRequest), Reads,
        "List archived cards",
        "A board's archived cards, the latest archived first; page with after_id.";
    "create_card" => board_service_client::BoardServiceClient.create_card(CreateCardRequest), Writes,
        "Create a card",
        "Adds a card to a board: its title, a Markdown body, labels, people and a due date, in a column (the \
         first when empty), above before_id or at the bottom. Needs Manage Cards and Send Messages.";
    "update_card" => board_service_client::BoardServiceClient.update_card(UpdateCardRequest), Writes,
        "Change a card",
        "Changes what the change sets on a card: title, due date, labels, people, archived, or its body \
         (set_body with content and the whole list of attachments). Needs Manage Cards.";
    "move_card" => board_service_client::BoardServiceClient.move_card(MoveCardRequest), Writes,
        "Move a card",
        "Moves a card to a column, above before_id or at the bottom. Needs Manage Cards.";
    "set_board" => board_service_client::BoardServiceClient.set_board(SetBoardRequest), Writes,
        "Set a board's columns and labels",
        "Replaces a board's columns and labels with the lists given, in order (no id for new ones). A column \
         removed must hold no cards. Needs Manage Channels.";
}

/// Whether there's a tool by that name here.
pub(crate) fn has(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

/// Every tool here, as `tools/list` answers.
pub(crate) fn list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            let request = descriptor(tool.request);
            let required: Vec<&str> = request.get_field_by_name("server_id").map(|_| "server_id").into_iter().collect();
            json!({
                "name": tool.name,
                "title": tool.title,
                "description": tool.description,
                "inputSchema": {
                    "type": "object",
                    "properties": properties(&request, 0),
                    "required": required,
                    "additionalProperties": false
                },
                "annotations": {
                    "title": tool.title,
                    "readOnlyHint": tool.effect == Effect::Reads,
                    "destructiveHint": tool.effect == Effect::Destroys,
                    "idempotentHint": tool.effect == Effect::Reads,
                    "openWorldHint": false
                }
            })
        })
        .collect()
}

/// `tools/call` for a tool here: the server's MCP door first, then the call.
pub(crate) async fn call(cx: &Cx, name: &str, args: &Map<String, Value>) -> Result<Result<Value, Status>, RpcError> {
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        return Err(RpcError::invalid(format!("there's no tool {name:?}")));
    };
    let server_id = match args.get("server_id") {
        Some(Value::String(id)) if !id.trim().is_empty() => id.clone(),
        _ => return Err(RpcError::invalid("server_id is needed, as a string")),
    };
    if let Err(status) = tools::may_use(cx, &server_id).await {
        return Ok(Err(status));
    }
    run(cx, tool, Value::Object(args.clone())).await
}

/// The tool's arguments as its request: proto3 JSON, short enum names allowed.
fn request<T: prost::Message + Default>(name: &str, mut args: Value) -> Result<T, RpcError> {
    let desc = descriptor(name);
    expand(&mut args, &desc);
    let message = DynamicMessage::deserialize_with_options(desc, args, &DeserializeOptions::new())
        .map_err(|e| RpcError::invalid(e.to_string()))?;
    message.transcode_to().map_err(|e| RpcError::invalid(e.to_string()))
}

/// The call's response as proto3 JSON, with field names as in the .proto,
/// numbers as numbers and short enum names.
fn response(request: &str, message: &impl prost::Message) -> Value {
    let name = request.strip_suffix("Request").map(|stem| format!("{stem}Response")).expect("a request's name");
    let desc = descriptor(&name);
    let dynamic =
        DynamicMessage::decode(desc.clone(), message.encode_to_vec().as_slice()).expect("a message reads as itself");
    let options = SerializeOptions::new().use_proto_field_name(true).stringify_64_bit_integers(false);
    let mut value = dynamic.serialize_with_options(serde_json::value::Serializer, &options).expect("JSON");
    shorten(&mut value, &desc);
    match &value {
        Value::Object(map) if map.is_empty() => json!({ "done": true }),
        _ => value,
    }
}

// ── Schemas ─────────────────────────────────────────────────────────────────

/// Messages nested deeper than this are taken as any object.
const DEPTH: usize = 6;

/// A message's fields as JSON Schema properties, each described by its comment.
fn properties(message: &MessageDescriptor, depth: usize) -> Value {
    let mut properties = Map::new();
    for field in message.fields() {
        let mut schema = if field.is_map() {
            let value = field.kind().as_message().expect("a map entry").map_entry_value_field();
            json!({ "type": "object", "additionalProperties": kind_schema(&value, depth) })
        } else if field.is_list() {
            json!({ "type": "array", "items": kind_schema(&field, depth) })
        } else {
            kind_schema(&field, depth)
        };
        if let Some(comment) = comment(&field)
            && let Value::Object(map) = &mut schema
        {
            map.insert("description".into(), Value::from(comment));
        }
        properties.insert(field.name().to_string(), schema);
    }
    Value::Object(properties)
}

fn kind_schema(field: &FieldDescriptor, depth: usize) -> Value {
    match field.kind() {
        FieldKind::Bool => json!({ "type": "boolean" }),
        FieldKind::String | FieldKind::Bytes => json!({ "type": "string" }),
        FieldKind::Double | FieldKind::Float => json!({ "type": "number" }),
        FieldKind::Enum(e) => json!({ "type": "string", "enum": enum_names(&e) }),
        FieldKind::Message(m) => match m.full_name() {
            "google.protobuf.Timestamp" => json!({ "type": "string", "format": "date-time" }),
            "google.protobuf.Duration" | "google.protobuf.FieldMask" => json!({ "type": "string" }),
            _ if depth >= DEPTH => json!({ "type": "object" }),
            _ => json!({ "type": "object", "properties": properties(&m, depth + 1) }),
        },
        _ => json!({ "type": "integer" }),
    }
}

/// The comment above a field in its .proto, on one line.
fn comment(field: &FieldDescriptor) -> Option<String> {
    let file = field.parent_file().file_descriptor_proto().clone();
    let location = file.source_code_info.as_ref()?.location.iter().find(|l| l.path == field.path())?;
    let text = location.leading_comments.as_deref()?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

// ── Enums ───────────────────────────────────────────────────────────────────

/// What every value of an enum starts with: CHANNEL_TYPE_ for ChannelType.
fn prefix(e: &EnumDescriptor) -> String {
    let mut prefix = String::new();
    for (i, c) in e.name().chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            prefix.push('_');
        }
        prefix.push(c.to_ascii_uppercase());
    }
    prefix + "_"
}

fn short(e: &EnumDescriptor, full: &str) -> String {
    full.strip_prefix(&prefix(e)).unwrap_or(full).to_ascii_lowercase()
}

/// An enum's values as tools write them, leaving out UNSPECIFIED.
fn enum_names(e: &EnumDescriptor) -> Vec<String> {
    e.values().filter(|v| v.number() != 0).map(|v| short(e, v.name())).collect()
}

/// Applies `f` to every enum value in `value`, a `message` as JSON.
fn each_enum(value: &mut Value, message: &MessageDescriptor, f: &dyn Fn(&EnumDescriptor, &str) -> Option<String>) {
    let Value::Object(map) = value else { return };
    for field in message.fields() {
        let Some(v) = map.get_mut(field.name()) else { continue };
        let one = |v: &mut Value| match field.kind() {
            FieldKind::Enum(e) => {
                if let Value::String(s) = v
                    && let Some(new) = f(&e, s)
                {
                    *s = new;
                }
            }
            FieldKind::Message(m) => each_enum(v, &m, f),
            _ => {}
        };
        match v {
            Value::Array(items) if field.is_list() => items.iter_mut().for_each(one),
            Value::Object(entries) if field.is_map() => {
                let value_field = field.kind().as_message().expect("a map entry").map_entry_value_field();
                if let FieldKind::Message(m) = value_field.kind() {
                    entries.values_mut().for_each(|v| each_enum(v, &m, f));
                }
            }
            _ => one(v),
        }
    }
}

/// "text" becomes CHANNEL_TYPE_TEXT; full names stay as they are.
fn expand(value: &mut Value, message: &MessageDescriptor) {
    each_enum(value, message, &|e, s| {
        let full = format!("{}{}", prefix(e), s.to_ascii_uppercase());
        (e.get_value_by_name(s).is_none() && e.get_value_by_name(&full).is_some()).then_some(full)
    });
}

/// CHANNEL_TYPE_TEXT becomes "text".
fn shorten(value: &mut Value, message: &MessageDescriptor) {
    each_enum(value, message, &|e, s| Some(short(e, s)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_its_request_and_response() {
        for tool in TOOLS {
            descriptor(tool.request);
            descriptor(&tool.request.replace("Request", "Response"));
            assert!(descriptor(tool.request).get_field_by_name("server_id").is_some(), "{}", tool.name);
            assert!(!super::super::tools::has(tool.name), "{} is a tool already", tool.name);
        }
        assert_eq!(list().len(), TOOLS.len());
    }

    #[test]
    fn enums_read_short_and_write_short() {
        let req: pb::CreateChannelRequest = request(
            "fuwa.v1.CreateChannelRequest",
            json!({
                "server_id": "s",
                "name": "talk",
                "type": "category",
                "permission_overwrites": [
                    { "target_id": "s", "target": "role", "deny": ["view_channels", "PERMISSION_SEND_MESSAGES"] }
                ]
            }),
        )
        .ok()
        .unwrap();
        assert_eq!(req.r#type, pb::ChannelType::Category as i32);
        assert_eq!(
            req.permission_overwrites[0].deny,
            vec![pb::Permission::ViewChannels as i32, pb::Permission::SendMessages as i32]
        );
        let shown = response(
            "fuwa.v1.CreateChannelRequest",
            &pb::CreateChannelResponse {
                channel: Some(pb::Channel {
                    id: "c".into(),
                    r#type: pb::ChannelType::Voice as i32,
                    permission_overwrites: req.permission_overwrites,
                    ..Default::default()
                }),
            },
        );
        assert_eq!(shown["channel"]["type"], "voice");
        assert_eq!(shown["channel"]["permission_overwrites"][0]["deny"], json!(["view_channels", "send_messages"]));
        assert_eq!(
            prefix(
                &descriptor("fuwa.v1.AutoModRule")
                    .get_field_by_name("trigger")
                    .unwrap()
                    .kind()
                    .as_enum()
                    .unwrap()
                    .clone()
            ),
            "AUTO_MOD_TRIGGER_"
        );
    }

    #[test]
    fn unknown_fields_are_refused() {
        let bad = request::<pb::DeleteChannelRequest>(
            "fuwa.v1.DeleteChannelRequest",
            json!({ "server_id": "s", "channel": "c" }),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn schemas_carry_the_protos_comments() {
        let tools = list();
        let update = tools.iter().find(|t| t["name"] == "update_channel").unwrap();
        let parent = &update["inputSchema"]["properties"]["parent_id"];
        assert_eq!(parent["type"], "string");
        assert!(parent["description"].as_str().unwrap().contains("category"), "{parent}");
        let create = tools.iter().find(|t| t["name"] == "create_channel").unwrap();
        let kinds = &create["inputSchema"]["properties"]["type"]["enum"];
        assert!(kinds.as_array().unwrap().contains(&json!("category")));
        assert_eq!(update["inputSchema"]["required"], json!(["server_id"]));
    }
}
