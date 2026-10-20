# Signing in with an email code

People can sign in, or make an account, with a six-digit code the instance
emails them. No password is kept: the address is a sign-in method like
Google or waifu.dev, and an account can have one alongside its others
(Settings › Account › Sign-in methods › Add an email address).

## Turning it on

Email codes need a way to send mail. Pick one with `FUWA_MAIL_PROVIDER` and
say who mail comes from with `FUWA_MAIL_FROM`, then restart:

```sh
FUWA_MAIL_PROVIDER=resend          # smtp, ses, resend or off
FUWA_MAIL_FROM="Fuwa <no-reply@example.com>"
FUWA_RESEND_API_KEY=re_...
```

Mail settings are read only from the environment, so no credential is kept in
`node.db` or shown to anyone. Whether people may use email codes is a setting:
`FUWA_EMAIL_ACCOUNTS` sets the default and Instance settings › Sign-in
providers › Email codes changes it later.

| `FUWA_EMAIL_ACCOUNTS` | What it does |
| --- | --- |
| `open` (default) | Anyone can sign in with a code, and an address with no account gets a new one |
| `closed` | Only addresses already on an account can sign in |
| `off` | No email codes; addresses already added stay, but can't sign in until it's back on |

With no mail provider, email codes stay off whatever the setting says.

## Providers

### Resend

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_RESEND_API_KEY` | required | An API key with sending access |
| `FUWA_RESEND_URL` | `https://api.resend.com` | The API's address, for a proxy or a test server |

The `FUWA_MAIL_FROM` domain must be verified in Resend.

### Amazon SES (v2 API)

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_SES_REGION` | `AWS_REGION`, else `AWS_DEFAULT_REGION` | The SES region, like `eu-west-1` |
| `FUWA_SES_ACCESS_KEY_ID` | `AWS_ACCESS_KEY_ID` | The access key |
| `FUWA_SES_SECRET_ACCESS_KEY` | `AWS_SECRET_ACCESS_KEY` | Its secret |
| `FUWA_SES_SESSION_TOKEN` | `AWS_SESSION_TOKEN` | For temporary credentials |
| `FUWA_SES_ENDPOINT` | `https://email.<region>.amazonaws.com` | Another endpoint, for a VPC endpoint or a test server |
| `FUWA_SES_CONFIGURATION_SET` | unset | A configuration set to send with |

The key needs `ses:SendEmail`, and the `FUWA_MAIL_FROM` address or domain must
be a verified identity (out of the SES sandbox to mail anyone).

### SMTP

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_SMTP_HOST` | required | The mail server |
| `FUWA_SMTP_SECURITY` | `starttls` | `starttls` (upgrade a plain connection, required), `tls` (TLS from the start) or `none` (only for a relay on the same machine or network) |
| `FUWA_SMTP_PORT` | `587`, `465` for `tls`, `25` for `none` | The port |
| `FUWA_SMTP_USERNAME` | unset | Who to sign in to the server as; set with the password or not at all |
| `FUWA_SMTP_PASSWORD` | unset | Its password |

## How it works

1. The app sends the address. The instance makes a ticket and a six-digit
   code, keeps only their hashes for ten minutes, and mails the code. It
   answers the same whether or not the address has an account, so nobody can
   use it to find out who is on the instance.
2. The app sends the ticket and the code. A wrong code counts against the
   ticket, and five wrong ones end it. A right code signs in to the account
   with that address, asks for the two-step code if it's on, or, for a new
   address on an open instance, asks for a username and makes the account.

An address can be sent five codes in ten minutes, and the instance sends at
most a thousand in ten minutes in all, so it can't be used to flood an inbox or run up a
provider's bill. A mail that fails to send is said so at once, and its code is
dropped.

Adding an address to an account asks for the account's password or two-step
code first (as adding any sign-in method does), then a code sent to the new
address. An address can be on only one account, and an account has one
address at most.
