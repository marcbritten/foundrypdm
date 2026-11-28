use futures::StreamExt;
use pdm_core::PdmError;
use sqlx::Executor;
use sqlx::PgPool;

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_thumbnails.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_client_settings.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_comments.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_manufacturing.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_manufacturing_chain.sql");
const MIGRATION_0007: &str = include_str!("../migrations/0007_nc_outdated.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_printing_sheetmetal.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_sheet_material_standards.sql");
const MIGRATION_0010: &str = include_str!("../migrations/0010_archive.sql");
const MIGRATION_0011: &str = include_str!("../migrations/0011_derived_cad.sql");
const MIGRATION_0012: &str = include_str!("../migrations/0012_preview_errors.sql");
const MIGRATION_0013: &str = include_str!("../migrations/0013_part_number_prefixes.sql");
const MIGRATION_0014: &str = include_str!("../migrations/0014_lifecycle_seed.sql");
const MIGRATION_0015: &str = include_str!("../migrations/0015_project_image.sql");
const MIGRATION_0016: &str = include_str!("../migrations/0016_solidcraft_artifacts.sql");
const MIGRATION_0017: &str = include_str!("../migrations/0017_main_assembly.sql");
const MIGRATION_0018: &str = include_str!("../migrations/0018_onshape_folder_sync.sql");
const MIGRATION_0019: &str = include_str!("../migrations/0019_version_revisions.sql");
const MIGRATION_0020: &str = include_str!("../migrations/0020_tool_library.sql");
const MIGRATION_0021: &str = include_str!("../migrations/0021_nesting.sql");
const MIGRATION_0022: &str = include_str!("../migrations/0022_nest_default_unit.sql");
const MIGRATION_0023: &str = include_str!("../migrations/0023_nest_grain_spacing.sql");
const MIGRATION_0024: &str = include_str!("../migrations/0024_nest_sheet_part_options.sql");
const MIGRATION_0025: &str = include_str!("../migrations/0025_nest_sheet_defs.sql");
const MIGRATION_0026: &str = include_str!("../migrations/0026_server_secrets.sql");
const MIGRATION_0027: &str = include_str!("../migrations/0027_org_branding.sql");
const MIGRATION_0028: &str = include_str!("../migrations/0028_session_expiry.sql");
const MIGRATION_0030: &str = include_str!("../migrations/0030_audit_org_id.sql");

pub async fn run_migrations(pool: &PgPool) -> Result<(), PdmError> {
    let mut tx = pool.begin().await.map_err(PdmError::Db)?;
    // API and dedicated worker may start together. Serialize migration discovery
    // and application in this database, including creation of the tracking table.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(0x50444d_i64)
        .execute(&mut *tx)
        .await
        .map_err(PdmError::Db)?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version INTEGER PRIMARY KEY, \
             applied_at TIMESTAMPTZ NOT NULL DEFAULT now())",
    )
    .execute(&mut *tx)
    .await
    .map_err(PdmError::Db)?;

    let applied: Vec<i32> = sqlx::query_scalar("SELECT version FROM schema_migrations")
        .fetch_all(&mut *tx)
        .await
        .map_err(PdmError::Db)?;

    let migrations: &[(i32, &str)] = &[
        (1, MIGRATION_0001),
        (2, MIGRATION_0002),
        (3, MIGRATION_0003),
        (4, MIGRATION_0004),
        (5, MIGRATION_0005),
        (6, MIGRATION_0006),
        (7, MIGRATION_0007),
        (8, MIGRATION_0008),
        (9, MIGRATION_0009),
        (10, MIGRATION_0010),
        (11, MIGRATION_0011),
        (12, MIGRATION_0012),
        (13, MIGRATION_0013),
        (14, MIGRATION_0014),
        (15, MIGRATION_0015),
        (16, MIGRATION_0016),
        (17, MIGRATION_0017),
        (18, MIGRATION_0018),
        (19, MIGRATION_0019),
        (20, MIGRATION_0020),
        (21, MIGRATION_0021),
        (22, MIGRATION_0022),
        (23, MIGRATION_0023),
        (24, MIGRATION_0024),
        (25, MIGRATION_0025),
        (26, MIGRATION_0026),
        (27, MIGRATION_0027),
        (28, MIGRATION_0028),
        (30, MIGRATION_0030),
    ];
    for (version, sql) in migrations {
        if !applied.contains(version) {
            {
                let mut stream = tx.execute_many(*sql).map(|res| res.map_err(PdmError::Db));
                while let Some(res) = stream.next().await {
                    res?;
                }
            }
            sqlx::query("INSERT INTO schema_migrations (version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await
                .map_err(PdmError::Db)?;
            tracing::info!(version, "applied migration");
        }
    }

    tx.commit().await.map_err(PdmError::Db)?;
    Ok(())
}
