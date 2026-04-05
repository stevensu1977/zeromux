# P1: GitHub OAuth + User System + Session Isolation

## Overview

ZeroMux now supports GitHub OAuth authentication with whitelist-based access control. The system supports two auth modes:

- **OAuth mode**: GitHub login + JWT + SQLite user store + whitelist
- **Legacy mode**: Single shared password/token (backwards compatible)

The mode is auto-detected: if `--github-client-id` and `--github-client-secret` are provided, OAuth mode is enabled. Otherwise, legacy mode is used.

## New CLI Arguments

| Argument | Env Var | Description |
|----------|---------|-------------|
| `--github-client-id` | `GITHUB_CLIENT_ID` | GitHub OAuth App client ID |
| `--github-client-secret` | `GITHUB_CLIENT_SECRET` | GitHub OAuth App client secret |
| `--jwt-secret` | `ZEROMUX_JWT_SECRET` | JWT signing secret (auto-generated if omitted) |
| `--data-dir` | — | SQLite database directory (default: `~/.zeromux/`) |
| `--allowed-users` | `ZEROMUX_ALLOWED_USERS` | Pre-approved GitHub usernames, comma-separated |
| `--external-url` | `ZEROMUX_EXTERNAL_URL` | Public URL for OAuth callback (e.g. `https://myserver.com`) |

## User Lifecycle

```
GitHub OAuth login
       │
       ▼
  First user? ──yes──▶ admin + active
       │
       no
       │
  In --allowed-users? ──yes──▶ member + active
       │
       no
       │
       ▼
  member + pending ──(admin approves)──▶ member + active
```

- **First user** to log in automatically becomes `admin` with `active` status
- Users in `--allowed-users` list are auto-approved on first login
- All other users enter `pending` status and see a "Waiting for approval" page
- Admin can approve or remove users from the admin panel

## Session Isolation

- Each session has an `owner_id` field tied to the creating user
- Users can only see and manage their own sessions
- Admin users can see and manage all sessions

## API Changes

### New Public Endpoints (no auth required)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/auth/mode` | Returns `{oauth: bool, legacy: bool}` |
| GET | `/auth/github` | Redirects to GitHub OAuth |
| GET | `/auth/github/callback` | OAuth callback, sets JWT cookie |
| POST | `/auth/login` | Legacy password login |

### New Authenticated Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/me` | Current user info (works for pending users too) |
| GET | `/api/admin/users` | List all users (admin only) |
| PUT | `/api/admin/users/{id}/approve` | Approve pending user (admin only) |
| DELETE | `/api/admin/users/{id}` | Remove user (admin only) |

## Frontend Changes

- **Login page**: Shows "Sign in with GitHub" button when OAuth is enabled, token input when legacy is enabled, or both
- **Waiting page**: Pending users see their avatar + "Waiting for approval" message, polls `/api/me` every 5s for auto-redirect on approval
- **Admin panel**: Admin users see a Users icon in the sidebar header, opens overlay with pending/active user list and approve/remove actions
- **Sidebar header**: Shows user avatar and GitHub username instead of "ZeroMux" text

## Files Changed

### Backend (new)
- `src/db.rs` — SQLite database layer (users table, CRUD)
- `src/oauth.rs` — GitHub OAuth flow + JWT signing
- `src/admin.rs` — Admin user management API

### Backend (modified)
- `Cargo.toml` — Added rusqlite, reqwest, jsonwebtoken
- `src/main.rs` — New CLI args, DB init, AppState fields
- `src/auth.rs` — Rewritten: JWT + legacy dual mode, CurrentUser with status
- `src/web.rs` — New routes, session owner filtering, /api/me endpoint
- `src/session_manager.rs` — owner_id on sessions, filtered listing, is_owner()
- `src/ws_handler.rs` — Updated to use verify_ws_token()
- `src/acp/ws_handler.rs` — Updated to use verify_ws_token()

### Frontend (new)
- `src/components/WaitingPage.tsx` — Pending user waiting screen
- `src/components/AdminPanel.tsx` — User management overlay

### Frontend (modified)
- `src/lib/api.ts` — Auth mode detection, JWT cookie support, admin APIs
- `src/components/LoginPage.tsx` — GitHub OAuth button + legacy token input
- `src/components/Sidebar.tsx` — User avatar, admin panel trigger
- `src/App.tsx` — Three-state auth flow (unauthenticated → pending → active)

## Setup: GitHub OAuth App

1. Go to GitHub → Settings → Developer settings → OAuth Apps → New
2. Set **Authorization callback URL** to `https://your-server.com/auth/github/callback`
3. Copy Client ID and Client Secret
4. Start ZeroMux:

```bash
./zeromux \
  --github-client-id=YOUR_CLIENT_ID \
  --github-client-secret=YOUR_CLIENT_SECRET \
  --external-url=https://your-server.com \
  --allowed-users=teammate1,teammate2
```
