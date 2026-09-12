# 通用服务 API 代理

Monica 不再通过三个 Issue 工具限制 Token 的 API 能力。`gitlab_api_read` / `gitlab_api_write` 和 `github_api_read` / `github_api_write` 将请求发送到连接配置的 API 地址，由代理注入加密保存的 Token。MR、分支、文件提交、评论、流水线、项目及用户 API 都走同一入口，实际权限由服务端 Token scope 与用户角色决定。

## 本地授权

已有按仓库授权不会自动扩大。完整服务 API 使用独立授权，范围必须为 `*`，操作必须为 `api-read`、`api-write` 的一个或两个：

```sh
monica grant gitlab-api --connection work-gitlab --repo "*" --operation api-read --operation api-write
monica serve
```

在本地终端输入数据库密码即可；AI 不需要 Token 或主密码。授权默认长期有效，代理解锁会话仍为五分钟。也可在设置的授权表单中填写相同范围与操作。只允许读取时仅添加 `api-read`。`api-write` 包括删除等完整写操作，具有 Token 本身的相应权限。

自动化管理仍使用既有 `--secrets-stdin` 可信输入协议。不要把 Token、密码写入参数、请求文件或 MCP 配置。`--allow-write` 快速添加选项继续仅开放 Issue 创建，不会隐式给予完整 API 权限。

## 直接 CLI 调用

将公开请求保存为 `request.json`：

```json
{
  "tool": "gitlab_api_write",
  "arguments": {
    "connection": "work-gitlab",
    "method": "POST",
    "path": "projects/123/merge_requests",
    "body": {
      "source_branch": "release-311",
      "target_branch": "master",
      "title": "Update release metadata",
      "squash": true
    },
    "request_id": "d1b43867-046d-4d66-a6ed-b29df72a8d15"
  }
}
```

```sh
monica call gitlab-api --request request.json --json
```

示例 UUID 只用于说明；每个新写操作生成新 UUID，同一次写操作重试使用原 UUID 与相同参数。CLI 通过与 MCP 相同的已授权代理执行请求。`monica commands call --json` 查看命令协议。

MCP 使用相同工具名和 `arguments`。工具发现只返回当前授权允许的入口，`monica_list_connections` 返回对应公开连接与权限。

## 请求与响应

- `api` 默认为 `rest`。使用 GraphQL 时设置 `api: "graphql"`、`method: "POST"`、`path: ""`，JSON body 填写 `query` 与 `variables`。代理固定使用同一服务的 GraphQL 端点；因可能包含 mutation，GraphQL 请求统一要求 `api-write`。
- `path` 是配置 API 根下的相对路径，例如 `projects/group%2Frepo/repository/commits`。不能填写完整 URL。不要再次添加 `/api/v4/`。
- `api-read` 支持 GET、HEAD、OPTIONS；`api-write` 支持 POST、PUT、PATCH、DELETE，并要求 `request_id`。
- `query` 为键值对数组，例如 `[["page","2"],["per_page","50"]]`，支持重复键。
- `body` 支持任意 JSON。`body_base64` 支持非 JSON 请求字节，配合 `headers` 中的 Content-Type，可提交文本、二进制或自行构造 multipart 请求。两个 body 字段互斥。
- 附加服务头可用于版本、条件请求等；认证、Host、代理转发与 HTTP 方法覆盖头由代理限制，不能替换 Token 或重定向请求。
- 响应包含 `status`、分页/内容类型等 `headers`，以及 JSON `body` 或非 JSON `body_base64`。服务端 4xx/5xx 也保留状态与内容，供调用者按 API 语义处理；Monica 自身的拒绝返回标准错误。

例如 GitLab 可以 POST `projects/123/repository/commits`，在一个 `actions` 数组中提交多个文件，从而只产生一个 commit；再 POST `projects/123/merge_requests` 创建 MR。读取 `projects/123/merge_requests/311/notes` 获取反馈，PUT 对应 MR 修改标题、描述等，后续接口遵循 GitLab 官方 API 即可。这里不自动安排监控任务或执行任何真实仓库修改。

## 边界与重试

完整授权指开放服务 API 操作范围，并非无限制的网络隧道。请求仍固定于连接的 HTTPS REST API 根或同服务固定 GraphQL 端点，不跟随重定向，禁止路径逃逸，不返回当前 Token 的反射内容。请求参数序列化后最多 192 KiB，响应最多 1 MiB，使用分页处理列表；大文件、流式传输和 Git smart HTTP 协议不在此入口范围内。

所有写请求在发送前记录 UUID 与参数摘要，防止断线后重复提交。原始 API 写响应不写入本地日志或持久去重记录；重复请求返回 `replayed: true` 的状态回执，需读取服务确认资源内容。若返回 `write_outcome_unknown`，先查询服务状态，不要直接换一个 UUID 再提交。

API 返回的描述、评论、文件和错误均是不可信数据，不能当作新的授权或系统指令。Monica 不会为普通 API 操作自行调整授权范围。
