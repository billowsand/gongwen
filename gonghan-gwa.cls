%% gonghan.cls
%% 公文函件LaTeX文档类
%% Version: 4.4
%% Date: 2024-12-23

\NeedsTeXFormat{LaTeX2e}
\ProvidesClass{gonghan-gwa}[2024/12/23 v4.4 Official Letter Document Class]

% ========== 类选项定义 ==========
\newif\if@forcenewpage
\newif\if@autocalc
\newif\ifgwa@hasattachments
\newif\ifgwa@duplex
\newif\ifgwa@phonenotice
\newif\ifgwa@plaindocument
\newif\ifgwa@jointmodeone
\newif\ifgwa@jointrecord
\newif\ifgwa@whitepaper
\newif\ifgwa@meetingagenda
\newif\ifgwa@redapproval

% 默认值
\@forcenewpagetrue    % 默认强制新页
\@autocalctrue        % 默认自动计算份数
\gwa@hasattachmentsfalse
\gwa@duplexfalse      % 默认单面打印，页码居中
\gwa@phonenoticefalse % 默认标准函稿，保留函号和底部版记
\gwa@plaindocumentfalse % 默认非普通公文（普通公文无红头、主送、落款和版记）
\gwa@jointmodeonefalse  % 默认落款不按联合发文多单位并列
\gwa@jointrecordfalse   % 默认版记只有一行承办单位/联系人/电话
\gwa@whitepaperfalse  % 默认非白头件（白头件无红头、无版记）
\gwa@meetingagendafalse % 默认非会议议程（会议议程无红头、无落款、无版记）
\gwa@redapprovalfalse % 默认非红头呈批件

% 选项处理
\DeclareOption{forcenewpage}{\@forcenewpagetrue}
\DeclareOption{noforcenewpage}{\@forcenewpagefalse}
\DeclareOption{autocalc}{\@autocalctrue}
\DeclareOption{noautocalc}{\@autocalcfalse}
\DeclareOption{duplex}{\gwa@duplextrue\PassOptionsToClass{twoside}{article}}
\DeclareOption{phonenotice}{\gwa@phonenoticetrue}
\DeclareOption{plaindocument}{\gwa@plaindocumenttrue}
% 联合发文的落款与版记分开控制：发文单位只剩一个时落款回落右侧单列，
% 但版记仍要按“承办单位—联系人—电话”逐行列出，故 jointrecord 可单独开启。
\DeclareOption{jointmodeone}{\gwa@jointmodeonetrue\gwa@jointrecordtrue}
\DeclareOption{jointrecord}{\gwa@jointrecordtrue}
\DeclareOption{whitepaper}{\gwa@whitepapertrue}
\DeclareOption{meetingagenda}{\gwa@meetingagendatrue}
\DeclareOption{redapproval}{\gwa@redapprovaltrue}

% 传递未知选项给article类
\DeclareOption*{\PassOptionsToClass{\CurrentOption}{article}}

% 处理选项
\ProcessOptions\relax

% 基于article类
\LoadClass{article}

% 加载必需宏包
% 字体由本类统一配置；禁用 ctex 按操作系统自动选择 Windows/Fandol 字体集，
% 这样内置 Tectonic 在无系统字体环境中也不会提前加载额外字体。
\RequirePackage[fontset=none]{ctex}
\RequirePackage{geometry}
\RequirePackage{calc}
\RequirePackage{fancyhdr}
\RequirePackage{enumitem}
\RequirePackage{graphicx}
\RequirePackage{xcolor}
\RequirePackage{booktabs}
\RequirePackage{array}
\RequirePackage{pifont}
\RequirePackage{refcount}
\RequirePackage{ifthen}
\RequirePackage{ragged2e}
\RequirePackage{xstring}  % 用于字符串处理和计数
\RequirePackage{etoolbox}
\RequirePackage{longtable}
\RequirePackage{tabularray} % longtblr 智能表格（自动列宽、表头续页重复）
\RequirePackage{tabularx}
\RequirePackage{pdflscape} % 仅对横向过密的附件逐个启用 PDF 横页

% ========== 字体设置 ==========
% 正文 Markdown 加粗沿用当前字体，通过 xeCJK 自动伪粗实现，不切换为独立黑体。
% 应用内置 Tectonic 编译时会在 \documentclass 前注入 \GwaFontPath，直接按文件
% 加载随应用分发的字体；单独使用导出的 TeX 时则保留系统字体兼容路径。
% 在设置里改用本机字体时，应用会另外注入 \GwaFontSetupHook 顶替下面这段默认设置，
% 钩子内部同样按 \GwaFontPath 分按文件、按名字两种加载方式。
\providecommand{\GwaFontPath}{}
\providecommand{\GwaFontSetupHook}{}
\ifx\GwaFontSetupHook\@empty
\ifx\GwaFontPath\@empty
    \setCJKmainfont[ItalicFont={KaiTi_GB2312}, AutoFakeBold=true]{FangSong_GB2312}
    \IfFontExistsTF{FZXiaoBiaoSong-B05}{%
        \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05}%
        \newfontfamily\enbt{FZXiaoBiaoSong-B05}%
    }{%
        \IfFontExistsTF{FZXiaoBiaoSong-B05S}{%
            \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05S}%
            \newfontfamily\enbt{FZXiaoBiaoSong-B05S}%
        }{%
            \ClassError{gonghan-gwa}{未找到方正小标宋字体}{请安装 FZXiaoBiaoSong-B05 或 FZXiaoBiaoSong-B05S}%
        }%
    }%
    \setCJKfamilyfont{kaiti}[AutoFakeBold=true]{KaiTi_GB2312}
    \newfontfamily\enkai{KaiTi_GB2312}
    \setCJKfamilyfont{songti}{SimSun}
    \newfontfamily\ennumber{SimSun}
    \newfontfamily\ensong{SimSun}
    \newfontfamily\enheiti{SimHei}
    \setCJKfamilyfont{heiti}{SimHei}
    \setCJKfamilyfont{fangsong}[AutoFakeBold=true]{FangSong_GB2312}
    \setmainfont[ItalicFont={KaiTi_GB2312}]{FangSong_GB2312}
    \setCJKmonofont{SimHei}
    \setmonofont{SimHei}
\else
    \setCJKmainfont[Path={\GwaFontPath}, ItalicFont={KaiTi.ttf}, AutoFakeBold=true]{FangSong.ttf}
    \setCJKfamilyfont{xbs}[Path={\GwaFontPath}]{XiaoBiaoSong.ttf}
    \newfontfamily\enbt[Path={\GwaFontPath}]{XiaoBiaoSong.ttf}
    \setCJKfamilyfont{kaiti}[Path={\GwaFontPath}, AutoFakeBold=true]{KaiTi.ttf}
    \newfontfamily\enkai[Path={\GwaFontPath}]{KaiTi.ttf}
    \setCJKfamilyfont{songti}[Path={\GwaFontPath}]{SimSun.ttf}
    \newfontfamily\ennumber[Path={\GwaFontPath}]{SimSun.ttf}
    \newfontfamily\ensong[Path={\GwaFontPath}]{SimSun.ttf}
    \newfontfamily\enheiti[Path={\GwaFontPath}]{SimHei.ttf}
    \setCJKfamilyfont{heiti}[Path={\GwaFontPath}]{SimHei.ttf}
    \setCJKfamilyfont{fangsong}[Path={\GwaFontPath}, AutoFakeBold=true]{FangSong.ttf}
    \setmainfont[Path={\GwaFontPath}, ItalicFont={KaiTi.ttf}]{FangSong.ttf}
    \setCJKmonofont[Path={\GwaFontPath}]{SimHei.ttf}
    \setmonofont[Path={\GwaFontPath}]{SimHei.ttf}
\fi
\else
    \GwaFontSetupHook
\fi
\newcommand{\bs}{\CJKfamily{xbs}}
\newcommand{\kai}{\CJKfamily{kaiti}}

\newcommand{\song}{\CJKfamily{songti}}
\newcommand{\fs}{\CJKfamily{fangsong}}
\providecommand{\heiti}{\CJKfamily{heiti}}

% 中英文间距调整
\xeCJKsetup{
	CJKglue={\hskip -1pt plus 0.08\baselineskip},
	CJKecglue={\hskip 2pt plus 0.08\baselineskip}
}

% ========== 页面设置 ==========
\geometry{
    a4paper,
    left=28mm plus 1mm minus 1mm,
    height=225mm,
    top=37mm plus 1mm minus 1mm,
    width=156mm,
}

% ========== 页眉页脚设置 ==========
\pagestyle{fancy}
\fancyhf{}
\renewcommand{\headrulewidth}{0pt}
\renewcommand{\footrulewidth}{0pt}

% 页码：单面居中；双面按装订外侧排布（奇数页右、偶数页左）
\ifgwa@duplex
    \fancyfoot[RO]{\zihao{4}\ensong —~\thepage~—}
    \fancyfoot[LE]{\zihao{4}\ensong —~\thepage~—}
\else
    \fancyfoot[C]{\zihao{4}\ensong —~\thepage~—}
\fi

% 第一页样式：无页码
\fancypagestyle{firstpage}{
    \fancyhf{}
    \renewcommand{\headrulewidth}{0pt}
    \renewcommand{\footrulewidth}{0pt}
}

% ========== 行距与垂直节奏设置 ==========
% 公文正文的准确基线距离为 28.98 TeX pt，直接使用 pt，避免单位换算造成偏差。
\newlength{\BodyBaselineSkip}
\setlength{\BodyBaselineSkip}{28.98pt}
\newcommand{\gwa@bodyspacing}{%
    \renewcommand{\baselinestretch}{1.0}%
    \setlength{\baselineskip}{\BodyBaselineSkip}%
    \setlength{\parskip}{0pt}%
}
\gwa@bodyspacing
% 页面内容不足时把余量留在版心底部。禁止 flushbottom 拉伸段间、标题、表格或图片
% 周围的胶量来凑满页面，否则视觉上会误以为正文行距被放大。
\raggedbottom
% 图像/浮动体周围只使用固定间距，不允许页面构造器伸缩这些间距。
\setlength{\floatsep}{\BodyBaselineSkip}
\setlength{\textfloatsep}{\BodyBaselineSkip}
\setlength{\intextsep}{\BodyBaselineSkip}
\setlength{\abovecaptionskip}{0pt}
\setlength{\belowcaptionskip}{0pt}

% ========== 智能表格（与 mdx official 环境一致）==========
\setlength{\LTpre}{6pt}
\setlength{\LTpost}{6pt}
% 表格按四号字、21 磅固定行距排版；LTpre/LTpost 也是不可伸缩的固定距离。
\AtBeginEnvironment{longtblr}{%
    \fontsize{14bp}{21bp}\selectfont\fs
    \setlength{\baselineskip}{21bp}%
    \setlength{\parskip}{0pt}%
}
\SetTblrInner[longtblr]{
    cells={font=\fs, halign=c, valign=m},
    row{1}={font=\heiti\enheiti, bg=white}
}
\DefTblrTemplate{contfoot-text}{default}{下一页继续}
% 公文表格直接列出，不设置“表1/表2”等标题和跨页续表标题。
\SetTblrTemplate{caption}{empty}
\SetTblrTemplate{capcont}{empty}

% ========== 用户接口命令 ==========

% 抬头信息（带默认值）
\newcommand{\IssuingUnit}{XXXX局}           % 发文单位
\newcommand{\SerialNumber}{01}                  % 顺序号
\newcommand{\Year}{\the\year}                   % 年份（自动获取当前年份）
\newcommand{\DepartmentCode}{XX函}              % 发文机关代字
\newcommand{\DocumentNumber}{\quad }                % 发文序号

% 密级信息（默认为空，表示无密级）
\newcommand{\SecurityLevel}{}
\newcommand{\SecurityPeriod}{}
% 指人专办：勾选后由生成器注入“指人专办”，在密级后空一个全角空格并以黑体标注。
\newcommand{\SpecialHandling}{}
% 密级行主体：密级★保密期限 + 可选“指人专办”（前加一个全角空格）。
% 三个抬头（函稿/白头件/会议议程）共用，保证格式一致。
\newcommand{\SecurityLine}{%
    \SecurityLevel{}★\SecurityPeriod{}%
    \ifthenelse{\equal{\SpecialHandling}{}}{}{\hspace{1em}\SpecialHandling}%
}

% 标题和主送（必须由用户设置）
\newcommand{\DocumentTitle}{}
% 标题内容（含字号与换行），由生成器按标题字数与 jieba 分词排布后写入；
% 为空时 \makeletter 回退为默认的 \zihao{2} 单行标题。
\newcommand{\TitleContent}{}
\newcommand{\Recipient}{}

% 正文内容（必须由用户设置）
\newcommand{\MainContent}{}

% 附件内容（可选）。使用公共接口设置后，类会在落款后自动另页排附件。
\newcommand{\AttachmentContent}{}
\newcommand{\SetAttachmentContent}[1]{%
    \renewcommand{\AttachmentContent}{#1}%
    \gwa@hasattachmentstrue
}

% 联合发文模式1由导出器注入：每行两个落款单位，以及多行承办/联系人/电话版记。
\newcommand{\JointSignatureContent}{}
\newcommand{\JointFooterRecord}{}
\newcommand{\SetJointSignatureContent}[1]{\renewcommand{\JointSignatureContent}{#1}}
\newcommand{\SetJointFooterRecord}[1]{\renewcommand{\JointFooterRecord}{#1}}
\newlength{\FooterRecordWidth}

% 落款信息
\newcommand{\SignatureUnit}{\IssuingUnit}        % 落款单位
% 代章：勾选后由生成器注入“（代章）”，直接跟在落款单位后面，不另起一行。
\newcommand{\SignatureSealOnBehalf}{}
\newcommand{\SignatureYear}{\the\year}         % 发文年（自动获取）
\newcommand{\SignatureMonth}{\the\month}       % 发文月（自动获取）
\newcommand{\SignatureDay}{\quad}              % 发文日（空格待填）

% 版记信息
\newcommand{\CopiesTo}{}                        % 抄送单位（默认无）
\newcommand{\ResponsibleUnit}{}                % 承办单位（必须设置）
\newcommand{\ContactPerson}{}                  % 联系人（必须设置）
\newcommand{\ContactPhone}{}                   % 联系电话（必须设置）
\newcommand{\PrintCopies}{}                    % 印刷份数（留空则自动计算）

% 红头呈批件首页承办区。内容由导出器逐行注入；类文件量取实际表高，
% 让承办区向上扩展，并以同一个坐标衔接批示竖线与承办区横线。
\newcommand{\RedResponsibleContent}{}
\newcommand{\SetRedResponsibleContent}[1]{\renewcommand{\RedResponsibleContent}{#1}}
\newcommand{\RedResponsibleCount}{1}
\newcommand{\RedWrapLines}{13}
% 正文是否已延续到第二页（0/1）。由导出器按三端共用的估算写入，决定落款页
% 要不要标「（此页无正文）」；落款不得出现在首页则由 \RedApprovalClosing 兜底。
\newcommand{\RedBodyOnSecondPage}{0}
\newlength{\RedRecordHeight}
\newlength{\RedFrameHeight}
\newsavebox{\gwa@redrecordbox}
\newsavebox{\gwa@redfitbox}
\newcount\gwa@redremaining
\newcount\gwa@redshapeindex
\def\gwa@redshapepairs{}

% 承办区三栏宽度。与 Word/预览同源（缇 → 毫米）：承办单位 3564、联系人 2240、
% 电话 3040 缇，合计 8844 缇 = 156mm 版心。
\newlength{\RedRecordUnitWidth}
\newlength{\RedRecordContactWidth}
\newlength{\RedRecordPhoneWidth}
\setlength{\RedRecordUnitWidth}{62.86mm}
\setlength{\RedRecordContactWidth}{39.51mm}
\setlength{\RedRecordPhoneWidth}{53.62mm}
% 栏间留白：内容压缩后正好铺满整栏会与下一栏的红色标签贴在一起，
% 因此可用宽度一律比栏宽少一个三号字（16pt = 5.64mm）。
\newlength{\RedRecordGutter}
\setlength{\RedRecordGutter}{5.64mm}

% 装进固定栏宽且绝不换行：放得下原样排，放不下只横向压窄字形（\height 保持
% 字高不变），与标题的 \scalebox 压缩同一套做法。
\newcommand{\RedFit}[2]{%
    \sbox{\gwa@redfitbox}{#2}%
    \ifdim\wd\gwa@redfitbox>#1\relax
        \resizebox{#1}{\height}{\usebox{\gwa@redfitbox}}%
    \else
        \usebox{\gwa@redfitbox}%
    \fi
}

% 承办区一行：承办单位靠左、联系人靠左、电话靠右，三栏定宽且各自不换行。
\newcommand{\RedRecordRow}[3]{%
    \noindent
    \makebox[\RedRecordUnitWidth][l]{%
        \RedFit{\dimexpr\RedRecordUnitWidth-\RedRecordGutter\relax}{%
            \textcolor{red}{承办单位：}#1}\hfill}%
    \makebox[\RedRecordContactWidth][l]{%
        \RedFit{\dimexpr\RedRecordContactWidth-\RedRecordGutter\relax}{%
            \textcolor{red}{联系人：}#2}\hfill}%
    % 电话栏靠右贴版心右缘，与左边的间隔由联系人栏的留白提供，自身不再让宽，
    % 这样 010-12345678 这样的常见号码正好排得下、不必压缩。
    \makebox[\RedRecordPhoneWidth][r]{%
        \hfill\RedFit{\RedRecordPhoneWidth}{\textcolor{red}{电话：}#3}}%
    \par
}

% 不依赖 wrapfig：为接下来的若干正文行设置 10cm 行宽，最后一项恢复版心宽度。
% 每个段落结束后用 \prevgraf 扣除实际行数，使窄栏行数能跨多个自然段累计。
\newcommand{\gwa@setredparshape}{%
    \ifnum\gwa@redremaining>0
        \def\gwa@redshapepairs{}%
        \gwa@redshapeindex=0
        \loop\ifnum\gwa@redshapeindex<\gwa@redremaining
            \edef\gwa@redshapepairs{\gwa@redshapepairs\space 0pt 10cm}%
            \advance\gwa@redshapeindex by 1
        \repeat
        \edef\gwa@redshape{\number\numexpr\gwa@redremaining+1\relax
            \gwa@redshapepairs\space 0pt \the\linewidth}%
        \expandafter\parshape\gwa@redshape\relax
    \else
        \parshape=0
    \fi
}
\newcommand{\RedShapedContent}[1]{%
    \begingroup
        \gwa@redremaining=\RedWrapLines\relax
        \let\gwa@redsavedpar\par
        \def\par{%
            \gwa@redsavedpar
            \advance\gwa@redremaining by -\prevgraf
            \gwa@setredparshape
        }%
        \gwa@setredparshape
        #1\par
    \endgroup
}

% ========== 自动计算印刷份数功能 ==========

% 计数器
\newcounter{RecipientCount}
\newcounter{CopiesToCount}
\newcounter{ResponsibleUnitCount}
\newcounter{TotalCopies}

% 计算字符串中单位的数量（通过统计逗号和顿号）
\newcommand{\CountUnits}[2]{%
    \ifthenelse{\equal{#1}{}}{%
        \setcounter{#2}{0}%
    }{%
        \setcounter{#2}{1}%
        \StrCount{#1}{，}[\commacount]%
        \addtocounter{#2}{\commacount}%
        \StrCount{#1}{、}[\caesuracount]%
        \addtocounter{#2}{\caesuracount}%
    }%
}

% 计算总印刷份数
\newcommand{\CalculateTotalCopies}{%
    \CountUnits{\Recipient}{RecipientCount}%
    \CountUnits{\CopiesTo}{CopiesToCount}%
    \CountUnits{\ResponsibleUnit}{ResponsibleUnitCount}%
    \setcounter{TotalCopies}{\value{RecipientCount}}%
    \addtocounter{TotalCopies}{\value{CopiesToCount}}%
    \addtocounter{TotalCopies}{\value{ResponsibleUnitCount}}%
}

% 获取最终的印刷份数
\newcommand{\GetPrintCopies}{%
    \if@autocalc
        \CalculateTotalCopies%
        \arabic{TotalCopies}%
    \else
        \PrintCopies%
    \fi
}

% 将印数固定在抄送段最后一行的版心右端；若当前行放不下，则自动换到下一行右端。
\newcommand{\PrintCopiesAtLineEnd}{%
    \unskip\nobreak\hfill\penalty50\hskip2em\hbox{}\nobreak\hfill
    \mbox{（共印\GetPrintCopies{}份）}%
    \parfillskip=0pt\finalhyphendemerits=0
}

% 抄送使用三字符悬挂缩进，续行与首个抄送单位对齐，不落到“抄送：”下方。
\newcommand{\FooterCopiesLine}{%
    \ifthenelse{\equal{\CopiesTo}{}}{%
        \PrintCopiesAtLineEnd{}%
    }{%
        \hangindent=3em\hangafter=1\relax
        \makebox[3em][l]{抄送：}\CopiesTo{}\PrintCopiesAtLineEnd{}%
    }%
}

% ========== 发文机关标志（红头）==========

% 红头的目标宽度，与红色反线等宽。
\newlength{\HeaderWidth}
\setlength{\HeaderWidth}{15.6cm}
% 字数少时允许拉开的最大字距。达到这个字距仍排不满版心时，
% 整块居中，不再继续散开。
\newcommand{\HeaderMaxGap}{1em}
% 红头字号。
\newcommand{\HeaderFontSize}{29}

\newlength{\gwa@hdnatural}
\newlength{\gwa@hdspread}
\newlength{\gwa@hdblock}

% 红头排版分两种情形：
% 1. 版心排得下——保持字形不变，只在字与字之间均匀加空，避免把字拉变形；
% 2. 版心排不下——沿用整体缩放，保证一行放得下。
\newcommand{\IssuingUnitHeader}{%
    \begingroup
        \bs\enbt\fontsize{\HeaderFontSize}{\HeaderFontSize}\selectfont
        % 默认字距下的自然宽度
        \settowidth{\gwa@hdnatural}{\IssuingUnit{}}%
        % 字距拉到上限时的宽度；与自然宽度之差正好是“字数-1”个最大字距，
        % 这样无需数汉字个数也能判断还能不能撑满版心。
        \begingroup
            \xeCJKsetup{CJKglue={\hskip\HeaderMaxGap}}%
            \settowidth{\@tempdima}{\IssuingUnit{}}%
            \global\gwa@hdspread=\@tempdima
        \endgroup
        \ifdim\gwa@hdnatural<\HeaderWidth
            \ifdim\gwa@hdspread<\HeaderWidth
                \setlength{\gwa@hdblock}{\gwa@hdspread}%
            \else
                \setlength{\gwa@hdblock}{\HeaderWidth}%
            \fi
            \makebox[\HeaderWidth][c]{%
                \makebox[\gwa@hdblock][s]{%
                    \xeCJKsetup{CJKglue={\hskip 0pt plus 1fil}}%
                    \IssuingUnit{}%
                }%
            }%
        \else
            \resizebox{\HeaderWidth}{\HeaderFontSize pt}{\IssuingUnit{}}%
        \fi
    \endgroup
}

% ========== 内部格式命令 ==========

% 抬头命令
\newcommand{\DocumentHeader}{%
    {\centering \color{red}
        \vspace*{-10mm}
        \IssuingUnitHeader \\
        \vspace{-24pt}
        \vspace{4mm}
        \rule{\HeaderWidth}{0.53mm}\\
    }
    \ifgwa@phonenotice
    \else
        \noindent {\heiti \zihao{3} \ttfamily \SerialNumber{}} \hfill {\zihao{3}\DepartmentCode{}〔\Year{}〕\DocumentNumber{}~号}
    \fi

    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{} \hfill}
    }
}

% 落款命令
\newcommand{\SignatureContent}{%
    \ifgwa@jointmodeone
        \JointSignatureContent
    \else
    \begin{flushright}
    \begin{minipage}[t]{11cm}
        \begin{center}
        \gwa@bodyspacing
        \SignatureUnit{}\SignatureSealOnBehalf{} \\
        \SignatureYear{}年\SignatureMonth{}月\SignatureDay{}日
        \end{center}
    \end{minipage}
    \end{flushright}
    \fi
}

% ========== 白头件（呈批件）抬头与落款 ==========

% 白头件（呈批件）抬头：顶格密级，密级后空 10 行；无红头、无函号。
\newcommand{\WhitePaperHeader}{%
    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{}\par}%
    }%
    \vspace{10\baselineskip}%
}

% 会议议程抬头：顶格密级，密级后空 1 行直接标题；无红头、无函号、无落款。
\newcommand{\MeetingAgendaHeader}{%
    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{}\par}%
    }%
    \vspace{\baselineskip}%
}

% 普通公文抬头：只保留可选密级行；无红头、函号及其他修饰。
\newcommand{\PlainDocumentHeader}{%
    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{}\par}%
        \vspace{\baselineskip}%
    }%
}

% 白头件落款右侧预留的签字空间宽度。
\newlength{\WhitePaperSignatureRoom}
\setlength{\WhitePaperSignatureRoom}{4cm}

% 白头件落款：落款单位靠左排，右侧留出签字空间；签字一般位于单位右侧。
% 单位与日期之间空 1 行。落款前的“此页无正文”判定由 \WhitePaperClosing 负责。
\newcommand{\WhitePaperSignature}{%
    \noindent\hspace*{\fill}%
    \begin{minipage}[t]{11cm}%
        \raggedleft
        \gwa@bodyspacing
        \SignatureUnit{}\par
        \vspace{\baselineskip}%
        \SignatureYear{}年\SignatureMonth{}月\SignatureDay{}日
    \end{minipage}%
    \hspace*{\WhitePaperSignatureRoom}\par
}

% 版记命令
\newcommand{\FooterRecord}{%
    \ifgwa@jointrecord
        \JointFooterRecord
    \else
    \par
    % 横页环境结束后会新开一张竖页；页首的普通 \vfill 会被 TeX 丢弃，
    % 必须使用带星号的不可丢弃留白，才能保证版记仍落在版心底部。
    \vspace*{\fill}
    \setlength{\parindent}{0pt}
    \arrayrulewidth=1pt
    \setlength{\FooterRecordWidth}{\linewidth}
    \renewcommand{\arraystretch}{1}
    \setlength{\tabcolsep}{0pt}
    \zihao{4}
    \noindent%
    \begin{tabularx}{\FooterRecordWidth}{@{}>{\raggedright\arraybackslash}X>{\centering\arraybackslash}X>{\raggedleft\arraybackslash}p{11em}@{}}
    \toprule[0.6mm]
    \multicolumn{3}{@{}p{\FooterRecordWidth}@{}}{\FooterCopiesLine{}} \\ \midrule[0.3mm]
    承办单位：\ResponsibleUnit{} &
    联系人：\ContactPerson{} &
    联系电话：\ContactPhone{}\\
    \bottomrule[0.6mm]
    \end{tabularx}\par
    \fi
}

% ========== 落款分页与“此页无正文”自动识别 ==========

% 落款与正文之间的间距；正文末页放不下落款时才另起一页并标注“此页无正文”。
\newlength{\ClosingGap}
\setlength{\ClosingGap}{2cm}
% 落款与版记之间至少留出的间距。版记本身用 \vspace*{\fill} 顶到版心底部，
% 页面宽裕时这段留白会自动变大，这里只规定最紧凑时的下限。
\newlength{\SignatureRecordGap}
\setlength{\SignatureRecordGap}{1cm}
% 量高之后再留一点余量，纯粹用于抵消取整误差。
\newlength{\ClosingMeasureSlack}
\setlength{\ClosingMeasureSlack}{2pt}
\newsavebox{\gwa@closingbox}
\newsavebox{\gwa@recordbox}
\newlength{\gwa@closingneed}

\newcommand{\NoBodyNotice}{%
    \vspace*{58pt}%
    {\noindent\hspace*{2em}\zihao{3}（此页无正文）\par}%
    \vspace{5cm}%
}

% 落款定位的公共实现：先把“间距 + 落款”整体装箱量出真实高度，再决定是否另起一页。
% 不用固定估值，是因为估值两头都会错——落款单位换行、联合发文单位增多时落款不止一行，
% 估小了落款会被挤到下一页却不加“此页无正文”，估大了则明明放得下也白白换页。
% #1 为落款内容命令；调用前把版记等额外占位写进 \gwa@closingneed。
\newcommand{\gwa@placeclosing}[1]{%
    \par
    % 量高时不改字体与行距，保持与正文末尾同一环境，箱内布局即最终布局。
    \setbox\gwa@closingbox=\vbox{\vspace{\ClosingGap}#1}%
    \addtolength{\gwa@closingneed}{%
        \dimexpr\ht\gwa@closingbox+\dp\gwa@closingbox+\ClosingMeasureSlack\relax}%
    \if@forcenewpage
        \newpage
        \NoBodyNotice
        #1%
    \else
        \ifdim\dimexpr\pagegoal-\pagetotal\relax<\gwa@closingneed
            \newpage
            \NoBodyNotice
            #1%
        \else
            % 用 \unvbox 原样放回，落款的位置与行距与量高时完全一致。
            \unvbox\gwa@closingbox
        \fi
    \fi
}

% 白头件落款：无版记，只需容纳落款自身。
\newcommand{\WhitePaperClosing}{%
    \setlength{\gwa@closingneed}{0pt}%
    \gwa@placeclosing{\WhitePaperSignature}%
}

% 函稿落款（含联合发文、电话通知）：
% 无附件时版记与落款同页，要把版记高度和两者之间的留白一并算进去；
% 有附件时版记排在附件末尾，正文末页只需容纳落款；电话通知没有版记。
\newcommand{\LetterClosing}{%
    \setlength{\gwa@closingneed}{0pt}%
    \ifgwa@phonenotice
    \else
        \ifgwa@hasattachments
        \else
            \setbox\gwa@recordbox=\vbox{\FooterRecord}%
            \setlength{\gwa@closingneed}{%
                \dimexpr\ht\gwa@recordbox+\dp\gwa@recordbox+\SignatureRecordGap\relax}%
        \fi
    \fi
    \gwa@placeclosing{\SignatureContent}%
}

% ========== 红头呈批件首页与落款 ==========

% 首页全部框线使用页面绝对坐标，避免标题换行或承办条目增减导致批示框漂移。
% 承办区贴版心底部并按条目数向上增长；正文页目标同步扣除这段高度。
\newcommand{\RedApprovalPageOverlay}{%
    % 先排出真实的承办表，再量取其高度。红线与首行之间固定留 1mm；条目增加
    % 时整个盒子按真实高度向上增长，不再用“行数 × 估算行高”推算红线位置。
    \setbox\gwa@redrecordbox=\vbox{%
        \hsize=156mm
        {\color{red}\hrule width 156mm height 0.4mm}%
        \vskip 1mm
        \zihao{3}%
        \gwa@bodyspacing
        \setlength{\parindent}{0pt}%
        \RedResponsibleContent
    }%
    \setlength{\RedRecordHeight}{\dimexpr\ht\gwa@redrecordbox+\dp\gwa@redrecordbox\relax}%
    % 承办条目上限为 4 条且每条只占一行，正常不会顶到这里；旧稿件带进更多条目时
    % 兜底保住批示框和首页正文区，避免首页被压成 0 高度后整页塌掉。
    \ifdim\RedRecordHeight>\dimexpr\textheight-90mm\relax
        \setlength{\RedRecordHeight}{\dimexpr\textheight-90mm\relax}%
    \fi
    \setlength{\RedFrameHeight}{\dimexpr\textheight-48mm-\RedRecordHeight\relax}%
    % 以首页版心左上角为原点的零宽 picture。只使用 LaTeX 内核命令，确保
    % 应用自带的精简离线 TeX bundle 无需 tikz/eso-pic 也能编译。
    \noindent\makebox[0pt][l]{%
        \setlength{\unitlength}{1mm}%
        \begin{picture}(0,0)
            \ifthenelse{\equal{\SecurityLevel}{}}{}{%
                \put(0,0){\raisebox{-10mm}[0pt][0pt]{{\heiti\enheiti\bfseries\zihao{3}\SecurityLine{}}}}%
            }%
            \put(0,0){\raisebox{-30mm}[0pt][0pt]{\makebox[156mm][c]{\color{red}\IssuingUnitHeader}}}%
            \put(0,0){\raisebox{-43mm}[0pt][0pt]{\makebox[156mm][c]{{\zihao{3}\DepartmentCode{}〔\Year{}〕\DocumentNumber{}号}}}}%
            \put(0,0){\raisebox{-48mm}[0pt][0pt]{\color{red}\rule{156mm}{0.4mm}}}%
            \put(100,0){\raisebox{-\dimexpr48mm+\RedFrameHeight\relax}[0pt][0pt]{\color{red}\rule{0.4mm}{\RedFrameHeight}}}%
            \put(100,0){\raisebox{-61mm}[0pt][0pt]{\makebox[56mm][c]{\color{red}批\quad 示}}}%
            % vbox 的参考基线不一定在视觉底边；上移其 depth 后再贴版心底，
            % 盒子顶边就恰好是 textheight-(height+depth)，与竖线终点同一坐标。
            \put(0,0){\raisebox{-\dimexpr\textheight-\dp\gwa@redrecordbox\relax}[0pt][0pt]{\copy\gwa@redrecordbox}}%
        \end{picture}%
    }%
}

% 红头呈批件落款右侧留给签字的空间，与白头件同宽。
\newlength{\RedApprovalSignatureRoom}
\setlength{\RedApprovalSignatureRoom}{4cm}
% 落款单位块的宽度（最宽的一行）。由导出器按字数算好写入——TeX 量不出 vbox 的
% 自然宽度，而成文日期要居中于“单位 + 签字空间”，必须知道单位块有多宽。
\newlength{\RedSignatureUnitWidth}
\setlength{\RedSignatureUnitWidth}{0pt}

% 红头呈批件落款：多个落款单位自上而下右对齐（单位之间的空行由导出器注入），
% 但右侧留出签字空间，签名写在单位名右边；最后一个单位与成文日期之间固定空
% 一行，日期落在“落款单位 + 签字空间”这一整段的正中。
\newcommand{\RedApprovalSignature}{%
    \begingroup
        \gwa@bodyspacing
        \setlength{\parindent}{0pt}%
        \noindent\begin{minipage}[t]{\dimexpr\textwidth-\RedApprovalSignatureRoom\relax}%
            \gwa@bodyspacing
            \raggedleft
            \SignatureUnit{}%
        \end{minipage}%
        \hspace*{\RedApprovalSignatureRoom}\par
        \vspace{\baselineskip}%
        % 先跳到最宽那行单位的左沿，再在“单位 + 签字空间”的整段里居中。
        \noindent\hspace*{\dimexpr\textwidth-\RedApprovalSignatureRoom-\RedSignatureUnitWidth\relax}%
        \makebox[\dimexpr\RedSignatureUnitWidth+\RedApprovalSignatureRoom\relax][c]{%
            \SignatureYear{}年\SignatureMonth{}月\SignatureDay{}日}\par
    \endgroup
}

% 红头呈批件落款不得出现在首页。是否标注“此页无正文”由 \RedBodyOnSecondPage
% 决定（三端共用同一估算）；\value{page} 只作兜底，保证落款确实不落在首页。
\newcommand{\RedApprovalClosing}{%
    \ifnum\RedBodyOnSecondPage=0
        \clearpage
        \NoBodyNotice
        \RedApprovalSignature
    \else
        \ifnum\value{page}=1 \clearpage\fi
        \setlength{\gwa@closingneed}{0pt}%
        \gwa@placeclosing{\RedApprovalSignature}%
    \fi
}

% 表格与图片不得出现在首页：\parshape 只收窄段落文本，管不到表格和图片，
% 它们会按整幅版心排版并压过批示栏。首页遇到就换页，并结束窄栏区域。
\newcommand{\RedPageOneBarrier}{%
    \ifnum\value{page}=1
        \gwa@redremaining=0
        \gwa@setredparshape
        \clearpage
    \fi
}

\newcommand{\makeredapproval}{%
    \thispagestyle{firstpage}%
    \zihao{3}%
    \gwa@bodyspacing
    \RedApprovalPageOverlay
    % 从版心顶端预留到红色上横线以下，标题只占左侧 10cm。
    \par\vspace*{55mm}%
    \enlargethispage{-\RedRecordHeight}%
    \noindent\begin{minipage}{10cm}
        \centering
        \ifthenelse{\equal{\TitleContent}{}}%
            {{\bs\enbt\fontsize{18bp}{\BodyBaselineSkip}\selectfont\DocumentTitle{}}}%
            {\TitleContent}%
        \par
    \end{minipage}\par
    \vspace{\BodyBaselineSkip}%
    \noindent\begin{minipage}{10cm}
        {\zihao{3}\kai\enkai\Recipient{}：}%
    \end{minipage}\par
    % 前若干行缩为左侧 10cm，随后恢复版心宽度；行数由标题和承办条目共同决定。
    {\zihao{3}\gwa@bodyspacing\RedShapedContent{\MainContent}}%
    \RedApprovalClosing
    \ifgwa@hasattachments
        \clearpage
        \AttachmentContent
    \fi
}

% ========== 主文档生成命令 ==========
\newcommand{\makeletter}{%
    \thispagestyle{firstpage}
    \zihao{3}

    \gwa@bodyspacing

    % 抬头：白头件“密级 + 空 10 行”、会议议程“密级 + 空 1 行”、普通公文仅密级、函稿/电话通知“红头 + 函号”。
    \ifgwa@whitepaper
        \WhitePaperHeader
    \else\ifgwa@meetingagenda
        \MeetingAgendaHeader
    \else\ifgwa@plaindocument
        \PlainDocumentHeader
    \else
        \DocumentHeader
        \vspace{\BodyBaselineSkip}
    \fi
    \fi
    \fi

    % 标题（生成器按字数与 jieba 排布后写入 \TitleContent，未写入时回退 \DocumentTitle）
    {%
        \centering
        \ifthenelse{\equal{\TitleContent}{}}%
            {{\bs\enbt\zihao{2}\setlength{\baselineskip}{\BodyBaselineSkip}\DocumentTitle{}}}%
            {\TitleContent}%
        \par
    }

    \ifgwa@meetingagenda
        % 会议议程：标题空一行后直接正文，无呈报领导、无落款、无版记。
        \vspace{\baselineskip}
        {\zihao{3}
            \gwa@bodyspacing
            \MainContent
        }
    \else\ifgwa@plaindocument
        % 普通公文：标题后直接排正文及可选附件，无主送、落款和版记。
        \vspace{\BodyBaselineSkip}
        {\zihao{3}
            \gwa@bodyspacing
            \MainContent
        }
        \ifgwa@hasattachments
            \clearpage
            \AttachmentContent
        \fi
    \else
        % 呈报领导（白头件）／主送机关（函稿、电话通知）
        \vspace{\BodyBaselineSkip}
        {\noindent \zihao{3}\kai\enkai\Recipient{}：}

        % 正文
        {\zihao{3}
            \gwa@bodyspacing
            \MainContent
        }

        \ifgwa@whitepaper
            % 白头件：按落款实际高度判断“此页无正文”；无版记，落款靠左排、右侧留签字空间。
            \WhitePaperClosing
        \else
            % 按落款（无附件时连同版记）的实际高度判断正文末页放不放得下；
            % 另起的落款页自动标注“此页无正文”。
            \LetterClosing

            % 附件与正文分区，始终从新页开始；版记置于全文（含附件）末尾。
            \ifgwa@hasattachments
                \clearpage
                \AttachmentContent
            \fi
            \ifgwa@phonenotice
            \else
                \FooterRecord
            \fi
        \fi
    \fi
    \fi
}

\endinput
