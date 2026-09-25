//! S7 验收：**换术语包不影响数据**。
//!
//! 术语包只覆盖"显示用的标签"（表名、字段名的中英文），列名、类型、引用、数据文件
//! 一个字节都不该变。这条测试把"数据没变"用**字节级比对**钉死。
//!
//! 数据全部为合成数据。

use std::fs;

use cercis_core::store::Identity;
use cercis_core::table::Row;
use cercis_core::{Project, Terms};

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-terms-{name}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

const AGRI: &str = r#"
schema: cercis.terms/1
profile: agriculture
label_cn: 农业术语包
tables:
  units:
    label_cn: 小区
    label_en: Plot
    fields:
      unit_id: { label_cn: 小区编号 }
  observations:
    label_cn: 观测记录
"#;

const MED: &str = r#"
schema: cercis.terms/1
profile: medicine
label_cn: 医学术语包
tables:
  units:
    label_cn: 病区
  observations:
    label_cn: 检查结果
"#;

#[test]
fn switching_term_pack_changes_labels_only() {
    let dir = tmp("switch");
    let p = Project::create(&dir, "术语包验收（合成数据）", Identity::default()).unwrap();
    p.install_template(&cercis_core::Template::example_study())
        .unwrap();

    // 先写研究，再写它下面的小区（引用完整性）
    let mut study = Row::new();
    study.insert("study_id".into(), "ST-001".into());
    study.insert("name".into(), "合成示例研究".into());
    p.store
        .append_row("studies", &["study_id", "name"], study, "ST-001", "写入研究")
        .unwrap();

    let mut row = Row::new();
    row.insert("unit_id".into(), "U-001".into());
    row.insert("study_id".into(), "ST-001".into());
    row.insert("treatment_id".into(), "T1".into());
    p.store
        .append_row(
            "units",
            &["unit_id", "study_id", "treatment_id"],
            row,
            "U-001",
            "写入示例数据",
        )
        .unwrap();

    // 装术语包之前：数据文件的字节快照
    let csv_before = fs::read(dir.join("tables/units.csv")).unwrap();
    let template_before = p.template_with_terms().unwrap().0;
    assert_eq!(template_before.table("units").unwrap().label_cn, "实验单元");

    // 装农业术语包
    p.install_terms(&Terms::from_yaml(AGRI).unwrap()).unwrap();

    let (t_agri, pack) = p.template_with_terms().unwrap();
    let pack = pack.expect("应当读到术语包");
    assert_eq!(pack.profile, "agriculture");
    assert_eq!(t_agri.table("units").unwrap().label_cn, "小区");
    assert_eq!(t_agri.table("units").unwrap().label_en, "Plot");
    assert_eq!(t_agri.table("units").unwrap().fields[0].label_cn, "小区编号");
    assert_eq!(t_agri.table("observations").unwrap().label_cn, "观测记录");
    // 术语包没覆盖的表保持原样
    assert_eq!(t_agri.table("studies").unwrap().label_cn, "研究");

    // 关键断言：数据文件字节级没变；列名也没变
    let csv_after = fs::read(dir.join("tables/units.csv")).unwrap();
    assert_eq!(csv_before, csv_after, "换术语包不得改动数据文件");
    let cols: Vec<&str> = t_agri
        .table("units")
        .unwrap()
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(cols, vec!["unit_id", "study_id", "treatment_id", "notes"]);

    // 再换一套术语包：同样只改标签
    let csv_before2 = fs::read(dir.join("tables/units.csv")).unwrap();
    p.install_terms(&Terms::from_yaml(MED).unwrap()).unwrap();
    let (t_med, pack2) = p.template_with_terms().unwrap();
    assert_eq!(pack2.unwrap().profile, "medicine");
    assert_eq!(t_med.table("units").unwrap().label_cn, "病区");
    // 医学包没写 unit_id，字段标签回到模板原样
    assert_eq!(t_med.table("units").unwrap().fields[0].label_cn, "实验单元编号");
    assert_eq!(
        fs::read(dir.join("tables/units.csv")).unwrap(),
        csv_before2,
        "再次切换术语包也不得改动数据"
    );

    // 数据仍然可校验、可查询
    assert!(p.validate().unwrap().is_ok());
    assert_eq!(p.query("units", cercis_core::Query::new()).unwrap().len(), 1);

    // 切换动作本身要留痕（谁在什么时候换了哪套术语包）
    let log = p.store.read_change_log().unwrap();
    let terms_entries: Vec<_> = log.iter().filter(|e| e.target_table == "terms").collect();
    assert_eq!(terms_entries.len(), 2, "两次切换都该有流水");
    assert!(terms_entries[0].summary.contains("agriculture"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn installs_template_from_a_file() {
    let dir = tmp("from-file");
    let p = Project::create(&dir, "文件装模板（合成数据）", Identity::default()).unwrap();

    // 模拟科研工具包提供的模板文件
    let pack_dir = tmp("pack");
    fs::create_dir_all(&pack_dir).unwrap();
    let file = pack_dir.join("tiny-template.yaml");
    fs::write(
        &file,
        r#"
name: tiny-research
label_cn: 极小科研模板
terms_profile: agriculture
tables:
  - name: plots
    label_cn: 小区
    primary_key: plot_id
    fields:
      - name: plot_id
        label_cn: 小区编号
        type: text
        required: true
      - name: area_m2
        label_cn: 面积
        type: number
        unit: m2
"#,
    )
    .unwrap();

    let text = fs::read_to_string(&file).unwrap();
    let template = cercis_core::Template::from_yaml(&text).unwrap();
    let created = p.install_template(&template).unwrap();
    assert_eq!(created, vec!["plots".to_string()]);

    // 装进去之后：模板生效、表已建好、能写能查
    let loaded = p.template().unwrap();
    assert_eq!(loaded.name, "tiny-research");
    assert_eq!(loaded.terms_profile, "agriculture");
    assert!(dir.join("tables/plots.csv").exists());

    let mut row = Row::new();
    row.insert("plot_id".into(), "P-01".into());
    row.insert("area_m2".into(), "12.5".into());
    p.store
        .append_row("plots", &["plot_id", "area_m2"], row, "P-01", "写入")
        .unwrap();
    assert_eq!(p.query("plots", cercis_core::Query::new()).unwrap().len(), 1);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&pack_dir);
}
