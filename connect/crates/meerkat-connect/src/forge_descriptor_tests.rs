//! Source-authored controls only. HTTP and terminal peers are local fixtures;
//! the delegated integration packet covers the actual native input separately.
//! None are run here or qualify an installed wrapper or live frontend.
use super::*;
use super::super::descriptor::{self, Descriptor, Selected};
use axum::{Router, body::Bytes, extract::State as AxumState,
    http::{HeaderMap, Method, StatusCode, Uri}, response::IntoResponse};
use nix::{fcntl::{FcntlArg, fcntl}, pty::openpty,
    sys::termios::{Termios, tcgetattr}, unistd::write};
use std::{collections::VecDeque, os::fd::OwnedFd,
    sync::atomic::{AtomicBool}, time::Duration};
use tokio::time::{sleep, timeout};

const BOUND: Duration = Duration::from_secs(5);
const COOKIE: &str = "mobkit_console_0123456789abcdef";
// Private canaries must not overlap the public hexadecimal host/source IDs.
const NONCE: &str = "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn";
const SESSION: &str = "sssssssssssssssssssssssssssssssssssssssssss";
const PASSWORD: &str = "descriptor fixture password";

fn host() -> ForgeHost { ForgeHost { protocol:1, host_id:format!("forge:{}", "a".repeat(64)),
    application_id:"generic-fixture".into(), source_digest:"b".repeat(64) } }
fn declaration() -> Value { json!({"protocol":1,"host":host(),"private_fields":["PRIVATE_TOKEN"]}) }
fn decoded(value: &Value) -> Descriptor { descriptor::decode(200,value.to_string().as_bytes()).unwrap() }
fn raw(value: &str) -> Box<RawValue> { RawValue::from_string(value.into()).unwrap() }
fn authenticated(origin: &str) -> ForgeClient {
    let mut client=ForgeClient::new(origin).unwrap();
    let mut cookie=HeaderValue::from_str(&format!("{COOKIE}={SESSION}")).unwrap();
    cookie.set_sensitive(true); client.session=Some(cookie); client
}
fn capture(front: &ForgeFrontend, client: ForgeClient, value: &Value) {
    let token=Arc::new(());
    *front.binding.state.lock().unwrap()=State::Pending(token.clone());
    assert!(front.binding.bind_descriptor(&token,&client,decoded(value)).unwrap());
    *front.binding.state.lock().unwrap()=State::Captured { _captured:Captured { client,
        support:ForgeSupport::Declared { host:host() } } };
}
fn generic_schema(front: &ForgeFrontend) {
    let tools=front.tools();
    let tool=tools.iter().find(|tool|tool.name==FORGE_PREPARE).unwrap();
    let text=serde_json::to_string(&tool.input_schema).unwrap();
    assert!(text.contains("configuration") && text.contains("expected_revision"));
    assert!(!text.contains("telegram_user_id") && !text.contains("participant_id"));
}

#[test]
fn descriptor_decoder_is_closed_bounded_and_preserves_empty_declarations() {
    assert_eq!(decoded(&declaration()).private_fields,["PRIVATE_TOKEN".to_owned()].into());
    let mut empty=declaration(); empty["private_fields"]=json!([]);
    assert!(decoded(&empty).private_fields.is_empty());
    for (key,value) in [("private_fields",json!(["A","A"])),("private_fields",json!([1])),
        ("private_fields",Value::Null),("private_fields",json!({})),("protocol",json!(2)),("extra",json!(true))] {
        let mut body=declaration(); body[key]=value;
        assert!(descriptor::decode(200,body.to_string().as_bytes()).is_err());
    }
    let duplicate=declaration().to_string().replacen('{',"{\"protocol\":1,",1);
    assert!(descriptor::decode(200,duplicate.as_bytes()).is_err());
    let duplicate=declaration().to_string().replacen("\"host\":{","\"host\":{\"protocol\":1,",1);
    assert!(descriptor::decode(200,duplicate.as_bytes()).is_err());
    assert!(descriptor::decode(200,&vec![b' ';LIMIT+1]).is_err());
    assert!(matches!(descriptor::decode(404,b"{}"),Err(ForgeError::UnexpectedResponse)));
    for (status,value) in [(401,json!({"error":"Console login required."})),
        (409,json!({"detail":"The loaded application source changed. No frontend command was admitted."})),
        (501,json!({"detail":UNSUPPORTED})),
        (503,json!({"detail":"The application frontend owner is unavailable. No activation is claimed."}))] {
        let result=descriptor::decode(status,value.to_string().as_bytes());
        assert!(matches!(result,Err(ForgeError::SessionRequired|ForgeError::SourceChanged|ForgeError::DeclarationUnsupported|ForgeError::Unavailable)));
        let mut extra=value; extra["host"]=json!({});
        assert!(matches!(descriptor::decode(status,extra.to_string().as_bytes()),Err(ForgeError::UnexpectedResponse)));
    }
}

#[test]
fn deferred_cli_keeps_legacy_reading_locators_status_only_under_current_declarations() {
    let legacy=super::tests::reference(); let original=legacy.proposal.get().to_owned();
    let fields=["PRIVATE_TOKEN".to_owned()].into();
    let recipe=Recipe::DeferredApplication(std::sync::OnceLock::from(Selected {
        origin:legacy.origin.clone(),descriptor:Descriptor {host:legacy.host.clone(),private_fields:fields},
    }));
    recipe.reference(&legacy).unwrap();
    assert_eq!(legacy.proposal.get(),original);
    assert_eq!(recipe.reference_schema(),2);
    let mut page=recovery::Page {references:vec![legacy.clone()],skipped:0,more:false};
    restrict_page(&recipe,&mut page); assert_eq!(page.references.len(),1);
    let mut foreign=legacy.clone(); foreign.origin="http://127.0.0.1:9999".into();
    assert!(recipe.reference(&foreign).is_err());
    let mut altered=legacy.clone(); altered.proposal=raw("{}");
    assert!(recipe.reference(&altered).is_err());
    // Exact status validation remains Reading-owned, even though new commands
    // retain the generic schema. Locators never supply current declarations.
    let unknown=json!({"change_id":legacy.change_id,"status":"unknown","storage":"available",
        "activation":{"state":"unbound","binding":"not-implemented","application_started":false,
        "provider_observation":null,"native_admission":null,"reply_receipt":null}});
    recipe.status(&unknown,&legacy,&legacy.host).unwrap();
    let mut wrong=unknown.clone(); wrong["change_id"]=json!("different");
    assert!(recipe.status(&wrong,&legacy,&legacy.host).is_err());
    let static_application=Recipe::Application {expected:legacy.host.clone(),private_fields:BTreeSet::new()};
    assert!(static_application.reference(&legacy).is_err());
    let (front,presenter)=ForgeFrontend::with_recipe(recipe);
    *front.binding.state.lock().unwrap()=State::Captured {_captured:Captured {
        client:authenticated(&legacy.origin),support:ForgeSupport::Declared {host:legacy.host.clone()},
    }};
    let prepared=Arc::new(Prepared {reference:legacy.clone(),expires_at:now()+300,proof:"c".repeat(64)});
    front.binding.configuration.lock().unwrap().prepared.insert(legacy.change_id.clone(),prepared);
    let arguments=raw(&json!({"change_id":legacy.change_id}).to_string());
    // The controlled proposal otherwise passes the existing retain check.
    // Selecting its saved locator makes the same proposal status-only.
    assert!(front.retain_action(ToolCallView {id:"old",name:FORGE_SUBMIT,args:&arguments}).is_ok());
    front.binding.configuration.lock().unwrap().recovered=Some(legacy.clone());
    assert!(front.retain_action(ToolCallView {id:"old",name:FORGE_SUBMIT,args:&arguments}).is_err());
    generic_schema(&front); drop(presenter);
}

#[test]
fn cli_continuation_moves_private_client_without_reviving_spent_pair_or_proposals() {
    let (front,presenter)=crate::main_connector::standalone_forge(None).unwrap();
    front.claim_native_setup(&presenter).unwrap();
    capture(&front,authenticated("http://127.0.0.1:12345"),&declaration());
    let legacy=super::tests::reference();
    let prepared=Arc::new(Prepared {reference:legacy,expires_at:now()+300,proof:"c".repeat(64)});
    { let mut state=front.binding.configuration.lock().unwrap();
      state.inventory=Some(json!({"old":true})); state.prepared.insert("old".into(),prepared.clone());
      state.armed=Some(HeldForgeAction {call_id:"spent".into(),arguments:"{}".into(),prepared});
      state.spent_calls.insert("spent".into()); state.submitted.insert("old".into()); }
    let same=front.clone(); drop(presenter);
    assert!(crate::main_connector::standalone_forge(Some(same)).is_err(),"shared old custody refuses");
    let (fresh,presenter)=crate::main_connector::standalone_forge(Some(front)).unwrap();
    assert!(matches!(*fresh.binding.state.lock().unwrap(),State::Fresh {native_setup_claimed:false}));
    { let state=fresh.binding.configuration.lock().unwrap(); assert!(state.inventory.is_none()
        && state.prepared.is_empty() && state.armed.is_none() && state.spent_calls.contains("spent")
        && state.submitted.contains("old") && state.uncertain.is_none()); }
    let retained=fresh.binding.continued_client.lock().unwrap();
    assert_eq!(retained.as_ref().unwrap().literal_origin,"http://127.0.0.1:12345");
    assert_eq!(retained.as_ref().unwrap().session.as_ref().unwrap().to_str().unwrap(),format!("{COOKIE}={SESSION}"));
    drop(retained); generic_schema(&fresh);
    fresh.claim_native_setup(&presenter).unwrap(); assert!(fresh.claim_native_setup(&presenter).is_err());
    drop(presenter);
    for busy in [false,true] {
        let (front,presenter)=crate::main_connector::standalone_forge(None).unwrap();
        if busy {front.binding.configuration.lock().unwrap().busy=true;}
        else {*front.binding.state.lock().unwrap()=State::Pending(Arc::new(()));}
        drop(presenter); assert!(crate::main_connector::standalone_forge(Some(front)).is_err());
    }
    // The explicit constructors retain their existing recipes.
    let (reading,_)=ForgeFrontend::new(); assert_eq!(reading.binding.recipe.reference_schema(),1);
}

#[derive(Clone,Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Output { fn text(&self)->String {String::from_utf8(self.0.lock().unwrap().clone()).unwrap()} }
impl Write for Output {
    fn write(&mut self,b:&[u8])->io::Result<usize> {self.0.lock().unwrap().extend_from_slice(b);Ok(b.len())}
    fn flush(&mut self)->io::Result<()> {Ok(())}
}
struct Pty {master:OwnedFd,observer:OwnedFd,original:Termios,flags:i32,input:TerminalInput}
impl Pty {
    fn new()->Self {let pair=openpty(None,None).unwrap();let observer=pair.slave.try_clone().unwrap();
        Self {original:tcgetattr(&observer).unwrap(),flags:fcntl(&observer,FcntlArg::F_GETFL).unwrap(),
            master:pair.master,observer,input:TerminalInput::from_owned_fd(pair.slave)} }
    fn restored(&self)->bool {tcgetattr(&self.observer).unwrap()==self.original && fcntl(&self.observer,FcntlArg::F_GETFL).unwrap()==self.flags}
}
async fn shown(output:&Output,text:&str) {while !output.text().contains(text) {sleep(Duration::from_millis(2)).await;}}
fn send(fd:&OwnedFd,value:&str) {assert_eq!(write(fd,value.as_bytes()).unwrap(),value.len());}

#[derive(Clone)]
struct Fixture {origin:String,replies:Arc<Mutex<VecDeque<(u16,Value)>>>,calls:Arc<Mutex<Vec<String>>>,hold:Arc<AtomicBool>,entered:Arc<tokio::sync::Notify>,release:Arc<tokio::sync::Semaphore>,status:Arc<Mutex<Option<Value>>>}
struct Server {fixture:Fixture,stop:oneshot::Sender<()>,task:tokio::task::JoinHandle<()>}
impl Server {
    async fn new(replies:Vec<(u16,Value)>)->Self {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture=Fixture {origin:format!("http://{}",listener.local_addr().unwrap()),replies:Arc::new(Mutex::new(replies.into())),calls:Default::default(),hold:Default::default(),entered:Default::default(),release:Arc::new(tokio::sync::Semaphore::new(0)),status:Default::default()};
        let (stop,receive)=oneshot::channel();let app=Router::new().fallback(handler).with_state(fixture.clone());
        let task=tokio::spawn(async move {axum::serve(listener,app).with_graceful_shutdown(async move{let _=receive.await;}).await.unwrap();});
        Self {fixture,stop,task}
    }
    async fn finish(self)->Vec<String> {let _=self.stop.send(());self.task.await.unwrap();self.fixture.calls.lock().unwrap().clone()}
}
async fn handler(AxumState(f):AxumState<Fixture>,method:Method,uri:Uri,headers:HeaderMap,body:Bytes)->axum::response::Response {
    f.calls.lock().unwrap().push(format!("{method} {}",uri.path()));
    assert_eq!(headers.get("host").unwrap(),f.origin.strip_prefix("http://").unwrap());
    assert!(uri.query().is_none());
    if method==Method::GET && uri.path()=="/login" {
        assert!(!headers.contains_key("cookie"));
        let html=format!("<form method=\"post\" action=\"/login\"><input type=\"hidden\" name=\"nonce\" value=\"{NONCE}\"><input type=\"password\" name=\"password\"></form>");
        let mut response=(StatusCode::OK,[("content-type","text/html"),("cache-control","no-store")],html).into_response();
        response.headers_mut().insert("set-cookie",format!("{COOKIE}_login={NONCE}; HttpOnly; Max-Age=300; Path=/; SameSite=Strict").parse().unwrap());return response;
    }
    if method==Method::POST && uri.path()=="/login" {
        assert_eq!(headers.get("cookie").unwrap(),format!("{COOKIE}_login={NONCE}").as_str());
        assert_eq!(headers.get("origin").unwrap(),f.origin.as_str());
        let value=Url::parse(&format!("http://127.0.0.1/?{}",std::str::from_utf8(&body).unwrap())).unwrap();
        let form=value.query_pairs().into_owned().collect::<BTreeMap<_,_>>();
        assert_eq!(form.get("password").unwrap(),PASSWORD); assert_eq!(form.get("nonce").unwrap(),NONCE);
        let mut response=(StatusCode::SEE_OTHER,[("location","/console"),("cache-control","no-store")]).into_response();
        response.headers_mut().append("set-cookie",format!("{COOKIE}={SESSION}; HttpOnly; Max-Age=28800; Path=/; SameSite=Strict").parse().unwrap());
        response.headers_mut().append("set-cookie",format!("{COOKIE}_login=\"\"; Max-Age=0; Path=/; SameSite=Lax").parse().unwrap());return response;
    }
    assert_eq!(method,Method::GET);
    assert_eq!(headers.get("cookie").unwrap(),format!("{COOKIE}={SESSION}").as_str());
    if uri.path()=="/console/forge/frontends/changes/change1" {
        let observation=f.status.lock().unwrap().take().expect("exact retained change status only");
        return (StatusCode::OK,[("content-type","application/json"),("cache-control","no-store")],
            json!({"protocol":1,"host":host(),"observation":observation}).to_string()).into_response();
    }
    assert_eq!(uri.path(),"/console/forge/frontends/descriptor");
    f.entered.notify_one();
    if f.hold.load(Ordering::SeqCst) {f.release.acquire().await.unwrap().forget();}
    let (status,body)=f.replies.lock().unwrap().pop_front().expect("only declared descriptor reads");
    (StatusCode::from_u16(status).unwrap(),[("content-type","application/json"),("cache-control","no-store")],body.to_string()).into_response()
}

type Observation = Result<Result<(),tokio::time::error::Elapsed>,Box<dyn std::any::Any+Send>>;
// Own the presenter outside observation timeout/panic. An entered login POST
// remains inside this retained future and settles after the stop before return.
async fn protected(prompt:ForgePrompt,origin:&str,enter_origin:bool,enter_password:bool)->(Result<bool,ConnectError>,Output,Observation,bool) {
    let mut pty=Pty::new();let mut output=Output::default();let seen=output.clone();
    let (stop,receive)=oneshot::channel();let mut stop=Some(stop);
    let done=AtomicBool::new(false);
    let (result,observed) = {
    let mut result=None;
    let work=async {let result=present_forge_selection(prompt,&mut pty.input,&mut output,async{receive.await.map_err(|_|io::Error::other("fixture listener ended"))}).await;done.store(true,Ordering::SeqCst);result};
    tokio::pin!(work);
    let observed=std::panic::AssertUnwindSafe(timeout(BOUND,async {
        let human=async {
            if enter_origin {shown(&seen,"Forge Console origin").await;send(&pty.master,&format!("{origin}\n"));}
            if enter_password {shown(&seen,"Forge Console password (hidden)").await;send(&pty.master,&format!("{PASSWORD}\n"));}
        };
        let (value,())=tokio::join!(work.as_mut(),human);result=Some(value);
    })).catch_unwind().await;
    if !done.load(Ordering::SeqCst) {let _=stop.take().unwrap().send(());result=Some(work.as_mut().await);}
    (result,observed)
    };
    let restored=pty.restored();
    (result.unwrap_or_else(||Err(ended())),seen,observed,restored)
}
fn observation_ok(value:Observation) {match value {Ok(Ok(()))=>(),Ok(Err(e))=>panic!("fixture observation timed out: {e}"),Err(p)=>std::panic::resume_unwind(p)}}
async fn select(front:&ForgeFrontend,presenter:&mut ForgePresenter,origin:&str,enter_origin:bool,enter_password:bool)
    ->(Result<ToolDispatchOutcome,ToolError>,Result<bool,ConnectError>,Output,Observation,bool)
{
    let call=async {front.dispatch(ToolCallView{id:"selection",name:FORGE_SELECT_HOST,args:&raw("{}")}).await};
    let show=async {protected(presenter.next().await.unwrap(),origin,enter_origin,enter_password).await};
    let (tool,(presented,output,observed,restored))=tokio::join!(call,show);
    (tool,presented,output,observed,restored)
}

#[tokio::test]
async fn real_descriptor_selection_keeps_live_login_and_rechecks_on_continuation() {
    for secret in [PASSWORD,NONCE,SESSION] {
        assert!(!declaration().to_string().contains(secret),
            "private fixture canary overlaps the public descriptor");
    }
    for scenario in ["same","expired","changed-fields","changed-source"] {
        let mut next=declaration();
        if scenario=="changed-fields" {next["private_fields"]=json!(["OTHER_PRIVATE"]);}
        if scenario=="changed-source" {next["host"]["source_digest"]=json!("d".repeat(64));}
        let mut replies=vec![(200,declaration())];
        if scenario=="expired" {replies.push((401,json!({"error":"Console login required."})));}
        replies.push((200,next));
        let server=Server::new(replies).await;
        let (front,mut presenter)=crate::main_connector::standalone_forge(None).unwrap();
        let first=select(&front,&mut presenter,&server.fixture.origin,true,true).await;
        drop(presenter);
        let (front,mut presenter)=crate::main_connector::standalone_forge(Some(front)).unwrap();
        let second=select(&front,&mut presenter,&server.fixture.origin,false,scenario=="expired").await;
        let schema=front.tools().iter().find(|t|t.name==FORGE_PREPARE).unwrap().input_schema.clone();
        let calls=server.finish().await;
        observation_ok(first.3);observation_ok(second.3);
        assert!(first.0.is_ok() && first.1.unwrap() && first.4);
        assert!(second.1.unwrap() && second.4);
        assert_eq!(second.0.is_err(),scenario.starts_with("changed"));
        assert!(schema["properties"].get("configuration").is_some());
        for out in [first.2,second.2] {for secret in [PASSWORD,NONCE,SESSION] {assert!(!out.text().contains(secret));}}
        assert_eq!(calls.iter().filter(|c|*c=="POST /login").count(),if scenario=="expired"{2}else{1});
        assert!(calls.iter().all(|c|c.ends_with("/login")||c.ends_with("/descriptor")));
        assert!(!calls.iter().any(|c|c.ends_with("/prepare")||c.ends_with("/submit")));
    }
}

#[tokio::test]
async fn changed_descriptor_refuses_configuration_before_post_and_keeps_spent_submit() {
    for operation in ["inspect","prepare","submit"] {
        let mut changed=declaration(); changed["private_fields"]=json!(["NEW_PRIVATE"]);
        let server=Server::new(vec![(200,changed)]).await;
        let (front,_presenter)=crate::main_connector::standalone_forge(None).unwrap();
        capture(&front,authenticated(&server.fixture.origin),&declaration());
        let result=if operation=="inspect" {
            front.dispatch(ToolCallView{id:"inspect",name:FORGE_INSPECT,args:&raw("{}")}).await.map(|_|())
        } else {
            let reference=RecoveryReference {schema:2,origin:server.fixture.origin.clone(),host:host(),change_id:"change1".into(),
                proposal:raw(r#"{"change_id":"change1","configuration":{},"expected_revision":"opaque"}"#),
                application_private_fields:Some(["PRIVATE_TOKEN".to_owned()].into())};
            let request=if operation=="submit" {OperationRequest::Submit(Arc::new(Prepared {reference,expires_at:now()+300,proof:"c".repeat(64)}))}
                else {OperationRequest::ApplicationPrepare {body:raw(r#"{"configuration":{},"expected_revision":"opaque"}"#)}};
            {let mut state=front.binding.configuration.lock().unwrap();state.busy=true;state.uncertain=Some(None);
                state.spent_calls.insert("spent".into());state.submitted.insert("change1".into());}
            let (send,receive)=oneshot::channel();
            let effect=ForgeOperation {binding:front.binding.clone(),client:authenticated(&server.fixture.origin),host:host(),request:Some(request),entered:false,response:Some(send)};
            let presented=present_operation(effect,&mut Output::default(),std::future::pending()).await;
            let result=receive.await.unwrap();
            assert!(presented.unwrap()); result.map(|_|()).map_err(error)
        };
        let calls=server.finish().await;
        assert!(result.is_err());assert_eq!(calls,vec!["GET /console/forge/frontends/descriptor"]);
        let state=front.binding.configuration.lock().unwrap();assert!(state.inventory.is_none()&&state.prepared.is_empty()&&state.armed.is_none());
        if operation!="inspect" {assert!(state.uncertain.is_none()&&state.spent_calls.contains("spent")&&state.submitted.contains("change1"));}
    }
}

#[tokio::test]
async fn cancellation_during_descriptor_preflight_never_enters_the_configuration_post() {
    for caller_lost in [false,true] {
        let server=Server::new(vec![(200,declaration())]).await;
        server.fixture.hold.store(true,Ordering::SeqCst);
        let (front,_presenter)=crate::main_connector::standalone_forge(None).unwrap();
        capture(&front,authenticated(&server.fixture.origin),&declaration());
        {let mut state=front.binding.configuration.lock().unwrap();state.busy=true;state.uncertain=Some(None);}
        let (send,receive)=oneshot::channel();let mut receive=Some(receive);
        let effect=ForgeOperation {binding:front.binding.clone(),client:authenticated(&server.fixture.origin),host:host(),
            request:Some(OperationRequest::ApplicationPrepare {body:raw(r#"{"configuration":{},"expected_revision":"opaque"}"#)}),entered:false,response:Some(send)};
        let (stop,stopped)=oneshot::channel();let mut stop=Some(stop);let mut output=Output::default();
        let done=AtomicBool::new(false);let mut result=None;
        let observed={
            let presenting=async {let value=present_operation(effect,&mut output,async{stopped.await.map_err(|_|io::Error::other("fixture listener ended"))}).await;
                done.store(true,Ordering::SeqCst);value};
            tokio::pin!(presenting);
            let observed=std::panic::AssertUnwindSafe(timeout(BOUND,async {
                tokio::select! {biased;
                    value=presenting.as_mut()=>{result=Some(value);panic!("descriptor must suspend before cancellation");},
                    _=server.fixture.entered.notified()=>(),
                }
                if caller_lost {drop(receive.take());} else {let _=stop.take().unwrap().send(());}
                result=Some(presenting.as_mut().await);
            })).catch_unwind().await;
            // Release the fixture read on every schedule, then settle the same
            // retained presenter before propagating an observation failure.
            server.fixture.release.add_permits(1);
            if !done.load(Ordering::SeqCst) {if let Some(stop)=stop.take(){let _=stop.send(());}result=Some(presenting.as_mut().await);}
            observed
        };
        let calls=server.finish().await;
        observation_ok(observed);
        if caller_lost {assert!(result.unwrap().is_err());} else {assert!(!result.unwrap().unwrap());}
        assert_eq!(calls,vec!["GET /console/forge/frontends/descriptor"]);
        let state=front.binding.configuration.lock().unwrap();assert!(!state.busy && state.uncertain.is_none());
    }
}

#[tokio::test]
async fn continued_unknown_prepare_and_known_change_keep_only_existing_status_authority() {
    for known in [false,true] {
        let mut replies=vec![(200,declaration()),(200,declaration())];
        if known {replies.push((200,declaration()));}
        let server=Server::new(replies).await;
        let outcome=std::panic::AssertUnwindSafe(async {
            let (front,mut presenter)=crate::main_connector::standalone_forge(None).unwrap();
            let first=select(&front,&mut presenter,&server.fixture.origin,true,true).await;
            let reference=RecoveryReference {schema:2,origin:server.fixture.origin.clone(),host:host(),change_id:"change1".into(),
                proposal:raw(r#"{"change_id":"change1","configuration":{},"expected_revision":"opaque"}"#),
                application_private_fields:Some(["PRIVATE_TOKEN".to_owned()].into())};
            {let mut state=front.binding.configuration.lock().unwrap();
                state.uncertain=Some(known.then(||"change1".into()));
                state.spent_calls.insert("spent".into());state.submitted.insert("change1".into());
                if known {state.references.insert("change1".into(),reference.clone());}
                let prepared=Arc::new(Prepared {reference,expires_at:now()+300,proof:"c".repeat(64)});
                state.prepared.insert("change1".into(),prepared.clone());
                state.armed=Some(HeldForgeAction {call_id:"spent".into(),arguments:"{}".into(),prepared});
            }
            drop(presenter);
            // A predecessor continuation panic is reported only after the
            // same fixture server has been stopped and joined below.
            let (front,mut presenter)=crate::main_connector::standalone_forge(Some(front)).unwrap();
            let second=select(&front,&mut presenter,&server.fixture.origin,false,false).await;
            let dispatches={
                let work=std::panic::AssertUnwindSafe(async {
                    let denied_inspect=front.dispatch(ToolCallView{id:"inspect",name:FORGE_INSPECT,args:&raw("{}")}).await;
                    let denied_prepare=front.dispatch(ToolCallView{id:"prepare",name:FORGE_PREPARE,args:&raw(r#"{"configuration":{},"expected_revision":"opaque"}"#)}).await;
                    let denied_submit=front.dispatch(ToolCallView{id:"spent",name:FORGE_SUBMIT,args:&raw(r#"{"change_id":"change1"}"#)}).await;
                    *server.fixture.status.lock().unwrap()=Some(json!({"change_id":"change1","status":"unknown"}));
                    let status=front.dispatch(ToolCallView{id:"status",name:FORGE_STATUS,args:&raw(r#"{"change_id":"change1"}"#)}).await;
                    (denied_inspect,denied_prepare,denied_submit,status)
                }).catch_unwind();
                tokio::pin!(work);
                match timeout(BOUND,work.as_mut()).await {
                    Ok(result)=>result,
                    Err(elapsed)=>{
                        // A regression may enqueue an unentered operation.
                        // Close its exact unused receiver, then join the same
                        // dispatch future, including any bounded HTTP read.
                        drop(presenter);
                        let _=work.as_mut().await;
                        panic!("fixture dispatch observation timed out: {elapsed}");
                    }
                }
            };
            let dispatches=match dispatches {Ok(value)=>value,Err(p)=>std::panic::resume_unwind(p)};
            (front,first,second,dispatches)
        }).catch_unwind().await;
        // Selection retains its existing protected deadline and entered-work
        // settlement. Neither that owner nor the server is owned by the
        // dispatch observation timeout; preserve the primary failure last.
        let finished=std::panic::AssertUnwindSafe(server.finish()).catch_unwind().await;
        let (front,first,second,(denied_inspect,denied_prepare,denied_submit,status))=
            match outcome {Ok(value)=>value,Err(p)=>std::panic::resume_unwind(p)};
        observation_ok(first.3);observation_ok(second.3);
        let calls=match finished {Ok(value)=>value,Err(p)=>std::panic::resume_unwind(p)};
        assert!(first.0.is_ok()&&first.1.unwrap()&&first.4&&second.0.is_ok()&&second.1.unwrap()&&second.4);
        assert!(denied_inspect.is_err()&&denied_prepare.is_err()&&denied_submit.is_err());
        assert_eq!(status.is_ok(),known);
        let state=front.binding.configuration.lock().unwrap();
        assert_eq!(state.uncertain,Some(known.then(||"change1".into())));
        assert!(state.inventory.is_none()&&state.prepared.is_empty()&&state.armed.is_none());
        assert!(state.spent_calls.contains("spent")&&state.submitted.contains("change1"));
        assert_eq!(state.references.contains_key("change1"),known);
        assert_eq!(calls.iter().filter(|call|*call=="POST /login").count(),1);
        assert!(!calls.iter().any(|call|call.ends_with("/prepare")||call.ends_with("/submit")));
        assert_eq!(calls.iter().filter(|call|call.contains("/changes/")).count(),usize::from(known));
    }
}

#[tokio::test]
async fn terminal_success_and_refusal_handoffs_observe_interrupt_while_ack_is_held() {
    for refused in [false,true] {
        let (front,_presenter)=crate::main_connector::standalone_forge(None).unwrap();
        let token=Arc::new(());*front.binding.state.lock().unwrap()=State::Pending(token.clone());
        let (send,receive)=oneshot::channel();
        let mut prompt=ForgeSelection {binding:front.binding.clone(),token,response:Some(send),prior:None};
        let selected=if refused {Err(ForgeError::UnexpectedResponse)} else {Ok(Captured {
            client:authenticated("http://127.0.0.1:12345"),support:ForgeSupport::Declared {host:host()},
        })};
        let (stop,stopped)=oneshot::channel();let mut stop=Some(stop);
        let interrupt=async{stopped.await.map_err(|_|io::Error::other("fixture listener ended"))};tokio::pin!(interrupt);
        let done=AtomicBool::new(false);let mut result=None;let mut held=None;let mut selected_was_refused=None;
        let observed={
            let work=async{let value=prompt.terminal_reply(selected,interrupt.as_mut()).await;done.store(true,Ordering::SeqCst);value};
            tokio::pin!(work);
            let observed=std::panic::AssertUnwindSafe(timeout(BOUND,async {
                let response=tokio::select! {biased;
                    value=work.as_mut()=>{result=Some(value);panic!("handoff must await caller acknowledgment");},
                    response=receive=>response.unwrap(),
                };
                selected_was_refused=Some(response.0.is_err());held=Some(response.1);
                let _=stop.take().unwrap().send(());
                result=Some(work.as_mut().await);
            })).catch_unwind().await;
            // An old implementation that ignores the stop can still be joined:
            // release the exact held acknowledgment before settling/reporting.
            if !done.load(Ordering::SeqCst) {
                if let Some(stop)=stop.take(){let _=stop.send(());}
                if let Some(ack)=held.take(){let _=ack.send(());}
                result=Some(work.as_mut().await);
            }
            observed
        };
        drop(prompt);
        let late_ack=held.map(|ack|ack.send(()));
        observation_ok(observed);
        assert_eq!(selected_was_refused,Some(refused));
        assert!(!result.unwrap().unwrap());
        assert!(late_ack.is_some_and(|result|result.is_err()));
        assert!(matches!(*front.binding.state.lock().unwrap(),State::Retired));
    }
}
