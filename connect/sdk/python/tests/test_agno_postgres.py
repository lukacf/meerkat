"""Selected PostgreSQL metadata on an explicitly supplied disposable fixture.

Run through docs/verification/20261003-agno-postgres-batch/run-source-tests.py.
No database is inferred from libpq, the environment or a default TCP address.
"""
from __future__ import annotations
import asyncio
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, redirect_stderr, redirect_stdout
import csv
import hashlib
import inspect
import io
import json
import os
from pathlib import Path
import time
import unittest
from unittest.mock import patch

import psycopg
from psycopg import sql
from agno.tools.postgres import PostgresTools
from jsonschema import Draft202012Validator
from mcp import Client

from meerkat_toolkit.adapters import agno_transports
from meerkat_toolkit.adapters.agno import AgnoAdapterError
from meerkat_toolkit.adapters.agno_profiles import build_profile, validate_configuration, ProfileConfigError
from meerkat_toolkit.adapters.agno_mcp import create_server

OPS=['show_tables','describe_table']
REF='TOOLKIT_FIXTURE_PASSWORD'


def configuration(**updates):
    return {'socket_directory':'/tmp/nonexistent-selected-socket','port':65432,'database':'postgres',
            'user':'reader_a','password_env':REF,'schema':'schema_a','timeout':2,'max_rows':20,**updates}


class ConfigurationTests(unittest.TestCase):
    def test_exact_donor_bodies_and_inert_construction(self):
        import agno.tools.postgres as donor
        self.assertEqual(hashlib.sha256(Path(donor.__file__).read_bytes()).hexdigest(),'94be4e1ed53fe16b5f56ded1c023f6af9ab77bdf852730ff97a87d3dfa380b62')
        with patch.dict(os.environ,{REF:'selected-only'}), patch.object(psycopg,'connect',side_effect=AssertionError('I/O during describe')):
            owner=agno_transports.build_postgres(socket_directory='/tmp/selected',port=65432,database='selected',user='reader',password='selected-only',schema='selected',timeout=2,max_rows=20)
            self.assertIs(type(owner).show_tables,PostgresTools.show_tables)
            self.assertIs(type(owner).describe_table,PostgresTools.describe_table)
            owner.close()
            profile=build_profile('postgres',operations=OPS,config=configuration())
            self.addCleanup(profile.close)
            self.assertEqual([x['name'] for x in profile.descriptions],OPS)
            self.assertEqual([x['effect'] for x in profile.descriptions],['read','read'])

    def test_configuration_is_explicit_and_local_only(self):
        valid=configuration(); self.assertEqual(validate_configuration('postgres',operations=OPS,config=valid).config,valid)
        for key in valid:
            bad=dict(valid);del bad[key]
            with self.subTest(missing=key),self.assertRaises(ProfileConfigError):validate_configuration('postgres',operations=OPS,config=bad)
        for update in [{'host':'example.invalid'},{'socket_directory':'localhost'},{'socket_directory':'/tmp/a,/tmp/b'},
                       {'socket_directory':'/tmp/x\x00'},{'socket_directory':'/tmp/'+'a'*81},{'port':True},{'port':0},{'port':65536},
                       {'max_rows':0},{'max_rows':1001},{'max_rows':False},{'database':'\x00'},{'user':'a'*64},
                       {'schema':'雪'*22},{'password_env':'PGPASSWORD'},{'password_env':'bad name'},{'options':'-c role=admin'}]:
            with self.subTest(update=update),self.assertRaises(ProfileConfigError):validate_configuration('postgres',operations=OPS,config={**valid,**update})
        for op in ['run_query','inspect_query','summarize_table','export_table_to_path']:
            with self.subTest(op=op),self.assertRaises(ProfileConfigError):validate_configuration('postgres',operations=[op],config=valid)

    def test_ambient_connection_service_and_password_state_is_rejected(self):
        with patch.dict(os.environ,{REF:'chosen'}),patch.object(psycopg,'connect',side_effect=AssertionError('must fail before I/O')):
            for name in ['PGSERVICE','PGSERVICEFILE','PGSYSCONFDIR','PGPASSFILE','PGPASSWORD','PGHOST','PGHOSTADDR','PGDATABASE','PGUSER','PGOPTIONS','PGREQUIREAUTH']:
                with self.subTest(name=name),patch.dict(os.environ,{name:'provider-canary'}),self.assertRaises(ProfileConfigError):
                    build_profile('postgres',operations=OPS,config=configuration())
            profile=build_profile('postgres',operations=OPS,config=configuration());self.addCleanup(profile.close)
            with patch.dict(os.environ,{'PGSERVICE':'provider-canary'}),self.assertRaises(AgnoAdapterError) as error:
                profile.adapter.invoke('show_tables',{})
            self.assertNotIn('provider-canary',str(error.exception))

    def test_operation_schema_and_identifier_boundaries(self):
        with patch.dict(os.environ,{REF:'chosen'}): profile=build_profile('postgres',operations=OPS,config=configuration())
        self.addCleanup(profile.close)
        schema=profile.descriptions[1]['parameters'];validator=Draft202012Validator(schema)
        for value in ['',None,True,'a'*64,'\x00','\ud800']:
            with self.subTest(value=repr(value)):self.assertFalse(validator.is_valid({'table':value}))
        for value in ['alpha','comma,"\n雪','😀'*15]:self.assertTrue(validator.is_valid({'table':value}))
        self.assertFalse(validator.is_valid({'table':'alpha','query':'SELECT 1'}))
        with patch.object(psycopg,'connect',side_effect=AssertionError('invalid identifier reached I/O')):
            for value in ['雪'*22,'\x00','\ud800','']:
                with self.subTest(value=repr(value)),self.assertRaises(AgnoAdapterError):profile.adapter.invoke('describe_table',{'table':value})


@unittest.skipUnless(os.environ.get('TOOLKIT_POSTGRES_FIXTURE'),'requires the explicit disposable PostgreSQL fixture runner')
class DatabaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture=json.loads(Path(os.environ['TOOLKIT_POSTGRES_FIXTURE']).read_text())

    def setUp(self):
        f=self.fixture
        self.stack=ExitStack();self.addCleanup(self.stack.close)
        self.admin=psycopg.connect(host=f['socket_directory'],port=f['port'],dbname=f['database'],user=f['admin_user'],connect_timeout=2,autocommit=True)
        self.stack.callback(self.admin.close)
        self.owners=[]
        real=agno_transports.build_postgres
        def capture(**kw):
            owner=real(**kw);self.owners.append(owner);return owner
        self.stack.enter_context(patch.object(agno_transports,'build_postgres',side_effect=capture))

    def profile(self,account='a',**updates):
        f=self.fixture
        c=configuration(**{'socket_directory':f['socket_directory'],'port':f['port'],'database':f['database'],'user':f['user_'+account],'schema':f['schema_'+account],**updates})
        with patch.dict(os.environ,{REF:f['password_'+account]}):p=build_profile('postgres',operations=OPS,config=c)
        self.stack.callback(p.close)
        return p,self.owners[-1]

    def test_actual_metadata_csv_and_empty_result(self):
        p,owner=self.profile()
        rows=list(csv.reader(io.StringIO(p.adapter.invoke('show_tables',{}))))
        self.assertEqual(rows[0],['table_name']);self.assertEqual({r[0] for r in rows[1:]},{'alpha','comma,"\n雪'})
        self.assertEqual(p.adapter.invoke('describe_table',{'table':'alpha'}),'column_name,data_type,is_nullable\nid,integer,NO\nlabel,text,YES')
        quoted=list(csv.reader(io.StringIO(p.adapter.invoke('describe_table',{'table':'comma,"\n雪'}))))
        self.assertEqual(quoted[1],['line,"\n雪','integer','YES'])
        self.assertEqual(quoted[2],['carriage\ronly','text','YES'])
        self.assertEqual(p.adapter.invoke('describe_table',{'table':'absent'}),'Query returned no results.\nColumns: column_name, data_type, is_nullable')
        self.assertEqual(owner._connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)
        self.assertEqual(owner._connection.execute('SELECT name FROM pg_cursors').fetchall(),[])
        owner._connection.rollback()

    def test_two_real_accounts_and_schemas_remain_separate(self):
        a,oa=self.profile('a');b,ob=self.profile('b')
        with ThreadPoolExecutor(max_workers=2) as pool:
            fa=pool.submit(a.adapter.invoke,'show_tables',{});fb=pool.submit(b.adapter.invoke,'show_tables',{})
            av,bv=fa.result(timeout=5),fb.result(timeout=5)
        self.assertIn('alpha',av);self.assertNotIn('beta',av);self.assertEqual(bv,'table_name\nbeta')
        self.assertIsNot(oa._connection,ob._connection)
        for owner,account in [(oa,'a'),(ob,'b')]:
            c=owner._connection
            self.assertEqual(c.execute('SELECT current_user, current_database(), current_setting(\'transaction_read_only\'), current_setting(\'search_path\')').fetchone(),(self.fixture['user_'+account],self.fixture['database'],'on','pg_catalog'))
            self.assertFalse(c.info.pgconn.ssl_in_use);c.rollback()
        self.assertEqual(a.adapter.invoke('describe_table',{'table':'schema_b.beta'}),'Query returned no results.\nColumns: column_name, data_type, is_nullable')
        q,_=self.profile('a',schema='schema_b')
        self.assertEqual(q.adapter.invoke('show_tables',{}),'Query returned no results.\nColumns: table_name')

    def test_selected_password_detached_and_invalid_password_sanitized(self):
        p,owner=self.profile('a')
        with patch.dict(os.environ,{REF:self.fixture['password_b']}):self.assertIn('alpha',p.adapter.invoke('show_tables',{}))
        output=io.StringIO()
        with patch.dict(os.environ,{REF:'wrong-provider-canary'}):
            bad=build_profile('postgres',operations=OPS,config=configuration(socket_directory=self.fixture['socket_directory']))
        self.stack.callback(bad.close)
        with redirect_stdout(output),redirect_stderr(output),self.assertRaises(AgnoAdapterError) as error:bad.adapter.invoke('show_tables',{})
        self.assertEqual(error.exception.kind,'entrypoint_error')
        self.assertEqual(output.getvalue(),'');self.assertNotIn('provider-canary',str(error.exception));self.assertIsNone(self.owners[-1]._connection)

    def test_schema_and_table_names_remain_literal_parameters(self):
        p,_=self.profile('a',schema='雪=schema')
        self.assertEqual(p.adapter.invoke('show_tables',{}),'table_name\nunicode_table')
        normal,_=self.profile()
        self.assertTrue(normal.adapter.invoke('describe_table',{'table':"alpha'; DROP TABLE schema_a.alpha; --"}).startswith('Query returned no results.'))
        self.assertEqual(self.admin.execute("SELECT to_regclass('schema_a.alpha')").fetchone()[0],'schema_a.alpha')

    def test_row_bound_fails_without_partial_success_and_rolls_back(self):
        p,owner=self.profile(max_rows=1)
        with self.assertRaises(AgnoAdapterError):p.adapter.invoke('show_tables',{})
        self.assertEqual(owner._connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)
        with self.assertRaises(AgnoAdapterError):p.adapter.invoke('describe_table',{'table':'alpha'})
        self.assertEqual(owner._connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)
        self.assertTrue(p.adapter.invoke('describe_table',{'table':'missing'}).startswith('Query returned no results.'))
        q,_=self.profile('b',max_rows=1);self.assertEqual(q.adapter.invoke('show_tables',{}),'table_name\nbeta')

    def test_scram_required_and_database_dsn_not_expanded(self):
        for update in [{'user':self.fixture['admin_user']}, {'database':'host=/tmp/foreign dbname=postgres'}]:
            with self.subTest(update=update):
                p,owner=self.profile(**update)
                with self.assertRaises(AgnoAdapterError):p.adapter.invoke('show_tables',{})
                self.assertIsNone(owner._connection)

    def test_real_server_cursor_limits_fetch_before_materialization(self):
        p,owner=self.profile(max_rows=1)
        observed=[]
        real=psycopg.ServerCursor.fetchmany
        def bounded(cursor,size=0):
            self.assertIsInstance(cursor,psycopg.ServerCursor);observed.append(size)
            return real(cursor,size)
        with patch.object(psycopg.ServerCursor,'fetchmany',bounded),patch.object(psycopg.ServerCursor,'fetchall',side_effect=AssertionError('unbounded fetch')):
            with self.assertRaises(AgnoAdapterError):p.adapter.invoke('show_tables',{})
        self.assertEqual(observed,[2])
        self.assertEqual(owner._connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)

    def test_actual_read_only_transaction_and_retirement(self):
        p,owner=self.profile();p.adapter.invoke('show_tables',{});connection=owner._connection
        with self.assertRaises(psycopg.errors.ReadOnlySqlTransaction):connection.execute('CREATE TABLE forbidden_profile_write (id INTEGER)')
        connection.rollback();self.assertIn('alpha',p.adapter.invoke('show_tables',{}))
        pid=connection.info.backend_pid;p.close();p.close();self.assertTrue(connection.closed);self.assertIsNone(owner._connection)
        with self.assertRaises(AgnoAdapterError):p.adapter.invoke('show_tables',{})
        self.assertIsNone(self.admin.execute('SELECT state FROM pg_stat_activity WHERE pid=%s',(pid,)).fetchone())

    def lock_metadata(self):
        f=self.fixture
        lock=psycopg.connect(host=f['socket_directory'],port=f['port'],dbname=f['database'],user=f['admin_user'],connect_timeout=2)
        self.stack.callback(lock.close)
        lock.execute('LOCK TABLE pg_catalog.pg_class IN ACCESS EXCLUSIVE MODE')
        return lock

    def wait_active(self,connection):
        until=time.monotonic()+3
        while time.monotonic()<until:
            value=self.admin.execute('SELECT state, wait_event_type FROM pg_catalog.pg_stat_get_activity(%s)',(connection.info.backend_pid,)).fetchone()
            if value==('active','Lock'):return
            time.sleep(.02)
        self.fail('Owned metadata backend did not reach the actual PostgreSQL lock')

    def test_actual_statement_timeout_rolls_back_and_recovers(self):
        p,owner=self.profile(timeout=1);p.adapter.invoke('show_tables',{})
        lock=self.lock_metadata();started=time.monotonic()
        with self.assertRaises(AgnoAdapterError):p.adapter.invoke('show_tables',{})
        self.assertLess(time.monotonic()-started,3)
        self.assertEqual(owner._connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)
        lock.rollback();self.assertIn('alpha',p.adapter.invoke('show_tables',{}))

    def test_actual_driver_cancel_rolls_back_and_recovers(self):
        p,owner=self.profile(timeout=5);p.adapter.invoke('show_tables',{});lock=self.lock_metadata()
        with ThreadPoolExecutor(max_workers=1) as pool:
            work=pool.submit(p.adapter.invoke,'show_tables',{});self.wait_active(owner._connection);owner._connection.cancel()
            with self.assertRaises(AgnoAdapterError):work.result(timeout=3)
        self.assertEqual(owner._connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)
        lock.rollback();self.assertIn('alpha',p.adapter.invoke('show_tables',{}))

    def test_real_mcp_invalid_arguments_and_failure_are_sanitized(self):
        p,_=self.profile(max_rows=1)
        async def run():
            async with Client(create_server(p)) as client:
                tools=await client.list_tools();self.assertEqual({x.name for x in tools.tools},set(OPS))
                for op,args in [('show_tables',{'query':'SELECT provider_canary'}),('describe_table',{'table':'\x00'})]:
                    result=await client.call_tool(op,args);self.assertTrue(result.is_error);self.assertEqual(result.structured_content['error']['kind'],'invalid_arguments')
                result=await client.call_tool('show_tables',{});self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content['error']['kind'],'entrypoint_error');self.assertNotIn('reader_a',str(result))
                result=await client.call_tool('describe_table',{'table':'absent'});self.assertFalse(result.is_error)
        asyncio.run(run())

    def test_mcp_cancellation_joins_real_backend_before_close(self):
        p,owner=self.profile(timeout=1);p.adapter.invoke('show_tables',{});connection=owner._connection;lock=self.lock_metadata()
        async def run():
            server=create_server(p)
            # Invoke the actual MCP handler directly to observe task retirement,
            # avoiding a client timeout being mistaken for worker completion.
            from mcp import types
            task=asyncio.create_task(server._request_handlers['tools/call'].handler(None,types.CallToolRequestParams(name='show_tables',arguments={})))
            await asyncio.to_thread(self.wait_active,connection)
            task.cancel();await asyncio.sleep(.05);self.assertFalse(task.done());self.assertFalse(connection.closed)
            with self.assertRaises(asyncio.CancelledError):await task
            self.assertEqual(connection.info.transaction_status,psycopg.pq.TransactionStatus.IDLE)
            p.close();self.assertTrue(connection.closed)
        try:asyncio.run(run())
        finally:lock.rollback()


if __name__=='__main__':unittest.main()
