//! Varias cuentas de correo detrás de un único `MailConnector` (R13, D41–D44). Cada cuenta es
//! un conector independiente —el simulador, Gmail o Microsoft Graph— y este módulo antepone el
//! identificador de la cuenta a los IDs de mensaje y de borrador (`<cuenta>:<id>`) y a las
//! claves de idempotencia (`<cuenta>/<clave>`), como pide el contrato de correo, para que no
//! choquen entre cuentas. La lista de cuentas no contiene secretos: los tokens viven en el
//! almacén de credenciales con el nombre `mail/<cuenta>` (D31).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nestadmin_ai::CredentialStore;
use nestadmin_core::{Connector, ConnectorError};
use nestadmin_mail::{Gmail, Graph, Mailbox, OAuthConfig, TokenSource, Tokens};
use nestadmin_simulators::{
    codes, MailConnector, MailSimulator, Message, MessagePage, NewDraft, SendReceipt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Identificador de la cuenta simulada.
pub const SIMULATOR: &str = "simulador";
/// Mensajes leídos por cuenta en cada consulta si no se configura otro valor.
pub const DEFAULT_MAIL_LIMIT: usize = 25;
/// Límite superior configurable: evita leer un buzón entero por error.
pub const MAX_MAIL_LIMIT: usize = 500;
const ACCOUNTS_FILE: &str = "cuentas.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailProviderKind {
    Simulator,
    Google,
    Microsoft,
}

impl MailProviderKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Simulator => SIMULATOR,
            Self::Google => "google",
            Self::Microsoft => "microsoft",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Simulator => "el simulador",
            Self::Google => "Google",
            Self::Microsoft => "Microsoft",
        }
    }
}

/// Dirección sintética del buzón simulado (la de destino de sus mensajes de ejemplo).
pub const SIMULATOR_ADDRESS: &str = "administracion@nestadmin.test";

/// Cuenta guardada en `correo/cuentas.json`. Sin tokens. Desde D48 guarda la dirección del
/// buzón, que el proveedor da al iniciar sesión, para decir de qué buzón viene cada correo;
/// solo se guarda en este equipo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub provider: MailProviderKind,
    /// Nombre que el usuario da a la cuenta; solo para reconocerla en la ventana.
    pub label: String,
    /// Dirección del buzón; `None` si el proveedor no la dio o la cuenta es anterior a D48.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// Cuenta de la que vino un correo, copiada en la tarea al encolarlo para que siga constando
/// aunque la cuenta se desconecte o cambie de nombre después.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailOrigin {
    pub account: String,
    pub provider: MailProviderKind,
    pub label: String,
    pub address: Option<String>,
}

/// Cliente OAuth y dirección de la API de un proveedor real. En la aplicación salen de la
/// configuración del editor (D43); en las pruebas apuntan a servidores simulados.
#[derive(Debug, Clone)]
pub struct ProviderProfile {
    pub oauth: OAuthConfig,
    pub api_base: String,
}

impl ProviderProfile {
    pub fn google(client_id: &str, client_secret: &str) -> Self {
        Self {
            oauth: OAuthConfig::google(client_id, client_secret),
            api_base: nestadmin_mail::gmail::DEFAULT_BASE.into(),
        }
    }

    pub fn microsoft(client_id: &str) -> Self {
        Self {
            oauth: OAuthConfig::microsoft(client_id),
            api_base: nestadmin_mail::graph::DEFAULT_BASE.into(),
        }
    }
}

/// Proveedores reales que esta instalación puede usar; `None` si el editor no configuró su
/// cliente OAuth.
#[derive(Debug, Clone, Default)]
pub struct MailProfiles {
    pub google: Option<ProviderProfile>,
    pub microsoft: Option<ProviderProfile>,
}

impl MailProfiles {
    fn get(&self, kind: MailProviderKind) -> Option<&ProviderProfile> {
        match kind {
            MailProviderKind::Google => self.google.as_ref(),
            MailProviderKind::Microsoft => self.microsoft.as_ref(),
            MailProviderKind::Simulator => None,
        }
    }
}

/// Lo que necesitan las cuentas reales: dónde guardar tokens y con qué clientes OAuth.
#[derive(Clone)]
pub struct MailEnv {
    pub credentials: Arc<dyn CredentialStore>,
    pub profiles: MailProfiles,
}

impl MailEnv {
    /// Sin proveedores reales: solo el simulador.
    pub fn simulator_only() -> Self {
        Self {
            credentials: Arc::new(nestadmin_ai::MemoryCredentialStore::default()),
            profiles: MailProfiles::default(),
        }
    }
}

/// Error de un conector tal como lo ve la ventana: código del contrato y texto sin secretos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailIssue {
    pub code: String,
    pub message: String,
}

impl From<&ConnectorError> for MailIssue {
    fn from(error: &ConnectorError) -> Self {
        Self {
            code: error.code.clone(),
            message: error.message.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// Hay tokens guardados (o es el simulador).
    Connected,
    /// Sin tokens: hay que volver a iniciar sesión.
    SignedOut,
    /// El editor no configuró el cliente OAuth de este proveedor en esta instalación.
    NotConfigured,
    /// El almacén de credenciales no respondió.
    CredentialsUnavailable,
}

/// Resultado de la última consulta de una cuenta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountFetch {
    pub account: String,
    /// Mensajes leídos en esta consulta (como mucho, el límite).
    pub read: usize,
    /// Mensajes nuevos encolados.
    pub added: usize,
    pub error: Option<MailIssue>,
    /// Adjuntos de los mensajes leídos.
    pub attachments: AttachmentFetch,
}

/// Adjuntos tratados en una consulta. Un adjunto que falla se vuelve a intentar en la
/// siguiente consulta; uno ya importado no se importa otra vez.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AttachmentFetch {
    /// Adjuntos importados como documentos en esta consulta.
    pub imported: usize,
    /// Adjuntos de un tipo que NestAdmin no procesa (por ejemplo `.docx` o `.zip`).
    pub skipped: Vec<String>,
    /// Adjuntos que no se pudieron descargar o importar.
    pub failed: Vec<AttachmentIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentIssue {
    pub name: String,
    pub code: String,
    pub message: String,
}

/// Resultado de consultar todas las cuentas: el fallo de una no detiene a las demás.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FetchReport {
    pub added: usize,
    pub limit: usize,
    pub accounts: Vec<AccountFetch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: String,
    pub provider: MailProviderKind,
    pub label: String,
    /// Dirección del buzón; `None` si no consta.
    pub address: Option<String>,
    pub status: AccountStatus,
    pub last_fetch: Option<AccountFetch>,
}

/// Estado de la pantalla de cuentas.
#[derive(Debug, Clone, Serialize)]
pub struct MailAccountsView {
    pub accounts: Vec<AccountView>,
    pub limit: usize,
    pub google_configured: bool,
    pub microsoft_configured: bool,
}

type Dyn = Box<dyn MailConnector + Send>;

struct Account {
    record: AccountRecord,
    /// `Err` si el proveedor no está configurado o el registro local no se pudo abrir.
    connector: Result<Dyn, ConnectorError>,
    last_fetch: Option<AccountFetch>,
}

pub(crate) struct Accounts {
    dir: PathBuf,
    env: MailEnv,
    limit: usize,
    simulator: Arc<MailSimulator>,
    list: Vec<Account>,
}

fn issue(code: &str, message: impl Into<String>) -> ConnectorError {
    ConnectorError {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// Separa `<cuenta>:<id>`. Un ID sin cuenta es de una tarea anterior a las cuentas y
/// pertenece al simulador.
pub fn split_id(id: &str) -> (&str, &str) {
    match id.split_once(':') {
        Some((account, inner)) if valid_account_id(account) => (account, inner),
        _ => (SIMULATOR, id),
    }
}

pub fn join_id(account: &str, id: &str) -> String {
    format!("{account}:{id}")
}

/// Clave que recibe el conector de la cuenta.
pub fn scoped_key(account: &str, key: &str) -> String {
    format!("{account}/{key}")
}

fn valid_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn credential_name(account: &str) -> String {
    format!("mail/{account}")
}

/// El simulador se comparte con `App::simulators` para que las pruebas vean sus efectos.
struct SharedSimulator(Arc<MailSimulator>);

impl Connector for SharedSimulator {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        self.0.lookup(key)
    }
    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        self.0.execute(key, payload)
    }
}

impl MailConnector for SharedSimulator {
    fn list_messages(&self, cursor: Option<&str>) -> Result<MessagePage, ConnectorError> {
        self.0.list_messages(cursor)
    }
    fn create_draft(
        &self,
        message_id: &str,
        draft: &NewDraft,
        idempotency_key: &str,
    ) -> Result<String, ConnectorError> {
        self.0.create_draft(message_id, draft, idempotency_key)
    }
    fn send_draft(
        &self,
        draft_id: &str,
        idempotency_key: &str,
    ) -> Result<SendReceipt, ConnectorError> {
        self.0.send_draft(draft_id, idempotency_key)
    }
    fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, ConnectorError> {
        self.0.fetch_attachment(message_id, attachment_id)
    }
}

impl Accounts {
    /// Lee la lista de `dir/cuentas.json`. Sin archivo, solo el simulador, que es el
    /// comportamiento anterior a las cuentas reales.
    pub(crate) fn open(dir: &Path, simulator: MailSimulator, env: MailEnv) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let records: Vec<AccountRecord> = match fs::read_to_string(dir.join(ACCOUNTS_FILE)) {
            Ok(text) => {
                let mut records: Vec<AccountRecord> =
                    serde_json::from_str(&text).map_err(io::Error::other)?;
                // El simulador siempre tiene su dirección sintética, también en listas antiguas.
                for record in &mut records {
                    if record.provider == MailProviderKind::Simulator {
                        record.address = Some(SIMULATOR_ADDRESS.into());
                    }
                }
                records
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => vec![simulator_record()],
            Err(e) => return Err(e),
        };
        let mut accounts = Self {
            dir: dir.to_owned(),
            env,
            limit: DEFAULT_MAIL_LIMIT,
            simulator: Arc::new(simulator),
            list: Vec::new(),
        };
        accounts.list = records.into_iter().map(|r| accounts.build(r)).collect();
        Ok(accounts)
    }

    pub(crate) fn simulator(&self) -> &MailSimulator {
        &self.simulator
    }

    fn token_source(&self, record: &AccountRecord) -> Option<TokenSource> {
        let profile = self.env.profiles.get(record.provider)?;
        Some(TokenSource::new(
            profile.oauth.clone(),
            Arc::clone(&self.env.credentials),
            &record.id,
        ))
    }

    fn build(&self, record: AccountRecord) -> Account {
        let connector = match record.provider {
            MailProviderKind::Simulator => {
                Ok(Box::new(SharedSimulator(Arc::clone(&self.simulator))) as Dyn)
            }
            kind => match (self.env.profiles.get(kind), self.token_source(&record)) {
                (Some(profile), Some(tokens)) => {
                    let state = self.dir.join(format!("{}.json", record.id));
                    // Cada página pide como mucho el límite; los proveedores admiten hasta 100.
                    let page = self.limit.min(100);
                    if kind == MailProviderKind::Google {
                        Gmail::new(&profile.api_base, tokens, page)
                            .and_then(|p| Mailbox::open(p, &state))
                            .map(|m| Box::new(m) as Dyn)
                    } else {
                        Graph::new(&profile.api_base, tokens, page)
                            .and_then(|p| Mailbox::open(p, &state))
                            .map(|m| Box::new(m) as Dyn)
                    }
                }
                _ => Err(not_configured(kind)),
            },
        };
        Account {
            record,
            connector,
            last_fetch: None,
        }
    }

    fn save(&self) -> io::Result<()> {
        let records: Vec<&AccountRecord> = self.list.iter().map(|a| &a.record).collect();
        let text = serde_json::to_string_pretty(&records).map_err(io::Error::other)?;
        let path = self.dir.join(ACCOUNTS_FILE);
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text)?;
        fs::rename(temporary, path)
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    /// Cambia el límite de mensajes leídos por cuenta en cada consulta.
    pub(crate) fn set_limit(&mut self, limit: usize) -> Result<(), String> {
        if !(1..=MAX_MAIL_LIMIT).contains(&limit) {
            return Err(format!(
                "el límite de mensajes debe estar entre 1 y {MAX_MAIL_LIMIT}"
            ));
        }
        if limit != self.limit {
            self.limit = limit;
            let records: Vec<AccountRecord> = self.list.iter().map(|a| a.record.clone()).collect();
            let previous: Vec<Option<AccountFetch>> =
                self.list.iter().map(|a| a.last_fetch.clone()).collect();
            self.list = records.into_iter().map(|r| self.build(r)).collect();
            for (account, last) in self.list.iter_mut().zip(previous) {
                account.last_fetch = last;
            }
        }
        Ok(())
    }

    /// Pide al proveedor la dirección del buzón con los tokens recién guardados. Un fallo no
    /// impide conectar la cuenta: la dirección queda como «no consta».
    fn lookup_address(&self, record: &AccountRecord) -> Option<String> {
        let profile = self.env.profiles.get(record.provider)?;
        let tokens = self.token_source(record)?;
        let found = match record.provider {
            MailProviderKind::Google => {
                Gmail::new(&profile.api_base, tokens, 1).and_then(|g| g.address())
            }
            MailProviderKind::Microsoft => {
                Graph::new(&profile.api_base, tokens, 1).and_then(|g| g.address())
            }
            MailProviderKind::Simulator => return Some(SIMULATOR_ADDRESS.into()),
        };
        found.ok().flatten()
    }

    /// Origen de los correos de una cuenta, para guardarlo en sus tareas.
    pub(crate) fn origin(&self, id: &str) -> Option<MailOrigin> {
        let account = self.list.iter().find(|a| a.record.id == id)?;
        Some(MailOrigin {
            account: account.record.id.clone(),
            provider: account.record.provider,
            label: account.record.label.clone(),
            address: account.record.address.clone(),
        })
    }

    fn status(&self, account: &Account) -> AccountStatus {
        if account.record.provider == MailProviderKind::Simulator {
            return AccountStatus::Connected;
        }
        match self.token_source(&account.record) {
            None => AccountStatus::NotConfigured,
            Some(tokens) => match tokens.signed_in() {
                Ok(true) => AccountStatus::Connected,
                Ok(false) => AccountStatus::SignedOut,
                Err(_) => AccountStatus::CredentialsUnavailable,
            },
        }
    }

    fn view_of(&self, account: &Account) -> AccountView {
        AccountView {
            id: account.record.id.clone(),
            provider: account.record.provider,
            label: account.record.label.clone(),
            address: account.record.address.clone(),
            status: self.status(account),
            last_fetch: account.last_fetch.clone(),
        }
    }

    pub(crate) fn view(&self) -> MailAccountsView {
        MailAccountsView {
            accounts: self.list.iter().map(|a| self.view_of(a)).collect(),
            limit: self.limit,
            google_configured: self.env.profiles.google.is_some(),
            microsoft_configured: self.env.profiles.microsoft.is_some(),
        }
    }

    /// Cliente OAuth con el que la ventana inicia sesión en un proveedor.
    pub(crate) fn oauth(&self, kind: MailProviderKind) -> Result<OAuthConfig, ConnectorError> {
        self.env
            .profiles
            .get(kind)
            .map(|p| p.oauth.clone())
            .ok_or_else(|| not_configured(kind))
    }

    fn position(&self, id: &str) -> Result<usize, ConnectorError> {
        self.list
            .iter()
            .position(|a| a.record.id == id)
            .ok_or_else(|| {
                issue(
                    codes::NOT_FOUND,
                    format!("la cuenta {id} no está conectada en NestAdmin"),
                )
            })
    }

    fn next_id(&self, kind: MailProviderKind) -> String {
        let prefix = kind.prefix();
        let used = self
            .list
            .iter()
            .filter_map(|a| a.record.id.strip_prefix(prefix)?.strip_prefix('-'))
            .filter_map(|n| n.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("{prefix}-{}", used + 1)
    }

    /// Añade una cuenta tras iniciar sesión: guarda los tokens en el almacén y la cuenta en la
    /// lista. Si el almacén falla no se añade nada.
    pub(crate) fn add(
        &mut self,
        kind: MailProviderKind,
        label: &str,
        tokens: &Tokens,
    ) -> Result<AccountView, ConnectorError> {
        if kind == MailProviderKind::Simulator {
            return self.add_simulator();
        }
        let id = self.next_id(kind);
        let label = account_label(kind, &id, label);
        let mut record = AccountRecord {
            id,
            provider: kind,
            label,
            address: None,
        };
        let source = self
            .token_source(&record)
            .ok_or_else(|| not_configured(kind))?;
        source.save(tokens)?;
        record.address = self.lookup_address(&record);
        let account = self.build(record);
        self.list.push(account);
        if let Err(e) = self.save() {
            let account = self.list.pop();
            if let Some(account) = account {
                let _ = self
                    .env
                    .credentials
                    .delete(&credential_name(&account.record.id));
            }
            return Err(issue(
                codes::UNAVAILABLE,
                format!("no se pudo guardar la lista de cuentas: {e}"),
            ));
        }
        Ok(self.view_of(self.list.last().expect("cuenta recién añadida")))
    }

    /// Cambia el nombre visible de una cuenta. El identificador no cambia (está en los IDs de
    /// mensaje y en las claves de idempotencia). Vacío vuelve al nombre predeterminado; un
    /// nombre repetido se admite, porque la dirección del buzón las distingue (D48).
    pub(crate) fn rename(&mut self, id: &str, label: &str) -> Result<AccountView, ConnectorError> {
        let index = self.position(id)?;
        let record = &self.list[index].record;
        let label = account_label(record.provider, &record.id, label);
        let previous = std::mem::replace(&mut self.list[index].record.label, label);
        if let Err(e) = self.save() {
            self.list[index].record.label = previous;
            return Err(issue(
                codes::UNAVAILABLE,
                format!("no se pudo guardar la lista de cuentas: {e}"),
            ));
        }
        Ok(self.view_of(&self.list[index]))
    }

    /// Vuelve a iniciar sesión en una cuenta existente (por ejemplo, tras un `unauthorized`).
    pub(crate) fn reconnect(
        &mut self,
        id: &str,
        tokens: &Tokens,
    ) -> Result<AccountView, ConnectorError> {
        let index = self.position(id)?;
        let mut record = self.list[index].record.clone();
        let source = self
            .token_source(&record)
            .ok_or_else(|| not_configured(record.provider))?;
        source.save(tokens)?;
        // Si el proveedor no la da ahora, se conserva la que ya constaba.
        if let Some(address) = self.lookup_address(&record) {
            record.address = Some(address);
        }
        let changed = record != self.list[index].record;
        // Conector nuevo: el anterior pudo guardar en caché tokens ya inválidos.
        let mut account = self.build(record);
        account.last_fetch = None;
        self.list[index] = account;
        if changed {
            // La sesión ya está renovada; si la lista no se guarda, la dirección se vuelve a
            // pedir en el próximo inicio de sesión.
            let _ = self.save();
        }
        Ok(self.view_of(&self.list[index]))
    }

    pub(crate) fn add_simulator(&mut self) -> Result<AccountView, ConnectorError> {
        if self.position(SIMULATOR).is_err() {
            let account = self.build(simulator_record());
            self.list.insert(0, account);
            self.save().map_err(|e| {
                issue(
                    codes::UNAVAILABLE,
                    format!("no se pudo guardar la lista de cuentas: {e}"),
                )
            })?;
        }
        Ok(self.view_of(&self.list[self.position(SIMULATOR)?]))
    }

    /// Desconecta una cuenta: borra sus tokens del almacén y la quita de la lista. Si el
    /// almacén no permite borrarlos, la cuenta se mantiene y se informa.
    pub(crate) fn remove(&mut self, id: &str) -> Result<(), ConnectorError> {
        let index = self.position(id)?;
        let record = self.list[index].record.clone();
        if record.provider != MailProviderKind::Simulator {
            self.env
                .credentials
                .delete(&credential_name(&record.id))
                .map_err(|e| {
                    issue(
                        codes::UNAVAILABLE,
                        format!("no se pudieron borrar los tokens: {e}"),
                    )
                })?;
        }
        self.list.remove(index);
        if let Err(e) = self.save() {
            return Err(issue(
                codes::UNAVAILABLE,
                format!("tokens borrados, pero no se pudo guardar la lista de cuentas: {e}"),
            ));
        }
        if record.provider != MailProviderKind::Simulator {
            // El registro local de borradores ya no sirve sin la cuenta.
            let _ = fs::remove_file(self.dir.join(format!("{}.json", record.id)));
        }
        Ok(())
    }

    fn connector(&self, account: &str) -> Result<&dyn MailConnector, ConnectorError> {
        let index = self.position(account)?;
        match &self.list[index].connector {
            Ok(connector) => Ok(connector.as_ref()),
            Err(e) => Err(e.clone()),
        }
    }

    /// Los mensajes más recientes de una cuenta, como mucho `limit`, con IDs de cuenta.
    fn recent(&self, account: &str) -> Result<Vec<Message>, ConnectorError> {
        let connector = self.connector(account)?;
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        while all.len() < self.limit {
            let page = connector.list_messages(cursor.as_deref())?;
            all.extend(page.messages);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        all.truncate(self.limit);
        for message in &mut all {
            message.id = join_id(account, &message.id);
        }
        Ok(all)
    }

    /// Lee cada cuenta por separado; el error de una no impide leer las demás.
    pub(crate) fn read_all(&self) -> Vec<(String, Result<Vec<Message>, ConnectorError>)> {
        self.list
            .iter()
            .map(|a| (a.record.id.clone(), self.recent(&a.record.id)))
            .collect()
    }

    pub(crate) fn record_fetch(&mut self, fetch: AccountFetch) {
        if let Some(account) = self.list.iter_mut().find(|a| a.record.id == fetch.account) {
            account.last_fetch = Some(fetch);
        }
    }

    /// Busca un mensaje entre los recientes de su cuenta. Solo para tareas antiguas que no
    /// guardaron el mensaje al encolarse.
    pub(crate) fn find_message(&self, id: &str) -> Result<Option<Message>, ConnectorError> {
        let (account, _) = split_id(id);
        let wanted = if id.contains(':') {
            id.to_owned()
        } else {
            join_id(account, id)
        };
        Ok(self.recent(account)?.into_iter().find(|m| m.id == wanted))
    }

    /// Contenido de un adjunto de un mensaje con ID de cuenta (`<cuenta>:<mensaje>`).
    pub(crate) fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, ConnectorError> {
        let (account, inner) = split_id(message_id);
        self.connector(account)?
            .fetch_attachment(inner, attachment_id)
    }

    /// Crea el borrador en la cuenta del mensaje; devuelve `<cuenta>:<borrador>`.
    pub(crate) fn create_draft(
        &self,
        message_id: &str,
        draft: &NewDraft,
        key: &str,
    ) -> Result<String, ConnectorError> {
        let (account, inner) = split_id(message_id);
        let draft_id =
            self.connector(account)?
                .create_draft(inner, draft, &scoped_key(account, key))?;
        Ok(join_id(account, &draft_id))
    }
}

/// Nombre de una cuenta: el que da el usuario (como mucho 60 caracteres) o, si está vacío,
/// el predeterminado («Google 1», «Microsoft 2», «Correo simulado»).
fn account_label(kind: MailProviderKind, id: &str, label: &str) -> String {
    match label.trim() {
        "" if kind == MailProviderKind::Simulator => "Correo simulado".into(),
        "" => format!("{} {}", kind.name(), id.rsplit('-').next().unwrap_or("1")),
        text => text.chars().take(60).collect(),
    }
}

fn simulator_record() -> AccountRecord {
    AccountRecord {
        id: SIMULATOR.into(),
        provider: MailProviderKind::Simulator,
        label: "Correo simulado".into(),
        address: Some(SIMULATOR_ADDRESS.into()),
    }
}

fn not_configured(kind: MailProviderKind) -> ConnectorError {
    issue(
        codes::UNAVAILABLE,
        format!(
            "esta instalación no tiene configurado el cliente OAuth de {}",
            kind.name()
        ),
    )
}

/// El recibo se devuelve con los IDs de la cuenta, para que el historial diga de dónde salió.
fn scoped_receipt(account: &str, receipt: Value) -> Value {
    let mut receipt = receipt;
    for field in ["receipt_id", "draft_id", "message_id"] {
        if let Some(id) = receipt.get(field).and_then(Value::as_str) {
            receipt[field] = Value::String(join_id(account, id));
        }
    }
    receipt["account"] = Value::String(account.to_owned());
    receipt
}

/// Acción `send_draft` vista desde el núcleo. La carga útil lleva `<cuenta>:<borrador>`;
/// la clave del núcleo se pasa a la cuenta como `<cuenta>/<clave>`.
impl Connector for Accounts {
    /// Solo la cuenta que envió tiene la clave en su registro local; las demás responden
    /// `None` sin consultar al proveedor. Si la cuenta dueña no responde, el error se
    /// propaga y la acción sigue pendiente.
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        let mut failure = None;
        for account in &self.list {
            let Ok(connector) = &account.connector else {
                continue;
            };
            let id = &account.record.id;
            let mut found = connector.lookup(&scoped_key(id, key));
            // Envíos del simulador anteriores a las cuentas, con la clave sin prefijo.
            if id == SIMULATOR && matches!(found, Ok(None)) {
                found = connector.lookup(key);
            }
            match found {
                Ok(Some(receipt)) => return Ok(Some(scoped_receipt(id, receipt))),
                Ok(None) => {}
                Err(e) => failure = Some(e),
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(None),
        }
    }

    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        let draft = payload
            .get("draft_id")
            .and_then(Value::as_str)
            .ok_or_else(|| issue(codes::INVALID_INPUT, "falta draft_id en la carga útil"))?;
        let (account, inner) = split_id(draft);
        let receipt = self.connector(account)?.execute(
            &scoped_key(account, key),
            &serde_json::json!({ "draft_id": inner }),
        )?;
        Ok(scoped_receipt(account, receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_carry_the_account() {
        assert_eq!(split_id("google-1:18c2:af"), ("google-1", "18c2:af"));
        assert_eq!(split_id("M-0001"), (SIMULATOR, "M-0001"));
        // Un prefijo que no es un identificador de cuenta no se interpreta como tal.
        assert_eq!(split_id("AAMk=:x"), (SIMULATOR, "AAMk=:x"));
        assert_eq!(join_id("microsoft-2", "AAMk="), "microsoft-2:AAMk=");
        assert_eq!(
            scoped_key("google-1", "task-3/send_draft/v1"),
            "google-1/task-3/send_draft/v1"
        );
    }

    #[test]
    fn receipts_name_their_account() {
        let receipt = serde_json::json!({
            "receipt_id": "s-1", "draft_id": "d-1", "message_id": "m-1", "idempotency_key": "k"
        });
        let scoped = scoped_receipt("google-1", receipt);
        assert_eq!(scoped["receipt_id"], "google-1:s-1");
        assert_eq!(scoped["draft_id"], "google-1:d-1");
        assert_eq!(scoped["message_id"], "google-1:m-1");
        assert_eq!(scoped["account"], "google-1");
    }
}
